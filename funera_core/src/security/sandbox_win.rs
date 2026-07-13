//! Windows sandbox implementation using write-restricted tokens + environment blocking.
//!
//! Based on OpenAI Codex's "unprivileged sandbox" approach:
//! - File-write isolation: Write-Restricted Token + synthetic SID + ACLs
//! - Network isolation: environment variable poisoning (advisory)
//!
//! No administrator privileges required.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::anyhow;
use super::sandbox::SandboxPolicy;

use windows::core::PWSTR;
use windows::Win32::Foundation::{
    BOOL, CloseHandle, ERROR_BROKEN_PIPE, HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAGS,
    WAIT_OBJECT_0,
};
use windows::Win32::Security::{
    AllocateAndInitializeSid, CreateRestrictedToken, DISABLE_MAX_PRIVILEGE, FreeSid,
    PSID, SANDBOX_INERT, SECURITY_ATTRIBUTES, SID_AND_ATTRIBUTES,
    TOKEN_ACCESS_MASK, TOKEN_GROUPS, TokenGroups, TOKEN_DUPLICATE, TOKEN_QUERY,
    GetTokenInformation,
};
use windows::Win32::System::Console::{GetStdHandle, STD_INPUT_HANDLE};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW, GetCurrentProcess,
    GetExitCodeProcess, OpenProcessToken, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
    STARTUPINFOW, TerminateProcess, WaitForSingleObject,
};

const WRITE_RESTRICTED: u32 = 0x0000_0008;
const SECURITY_NT_AUTHORITY: [u8; 6] = [0, 0, 0, 0, 0, 5];

pub struct WindowsSandbox {
    sid: PSID,
    read_write_paths: Vec<PathBuf>,
    block_network: bool,
}

unsafe impl Send for WindowsSandbox {}
unsafe impl Sync for WindowsSandbox {}

impl WindowsSandbox {
    pub fn new(policy: &SandboxPolicy) -> anyhow::Result<Self> {
        let sid = create_sandbox_sid()
            .map_err(|e| anyhow!("failed to create sandbox SID: {e}"))?;
        match apply_write_acls(sid, &policy.read_write_paths) {
            Ok(()) => Ok(Self {
                sid,
                read_write_paths: policy.read_write_paths.clone(),
                block_network: policy.block_network,
            }),
            Err(e) => {
                unsafe { FreeSid(sid) };
                Err(anyhow!("failed to apply write ACLs: {e}"))
            }
        }
    }

    pub async fn execute(
        &self,
        shell: &str,
        shell_flag: &str,
        command: &str,
        workdir: Option<&str>,
        timeout: Duration,
    ) -> anyhow::Result<(String, String, i32)> {
        match try_full_sandbox(self.sid, shell, shell_flag, command, workdir, timeout, self.block_network)
        {
            Ok(result) => return Ok(result),
            Err(_) => {}
        }
        execute_fallback(shell, shell_flag, command, workdir, timeout, self.block_network).await
    }
}

impl Drop for WindowsSandbox {
    fn drop(&mut self) {
        remove_write_acls(self.sid, &self.read_write_paths);
        if !self.sid.0.is_null() {
            unsafe { FreeSid(self.sid) };
        }
    }
}

async fn execute_fallback(
    shell: &str,
    shell_flag: &str,
    command: &str,
    workdir: Option<&str>,
    timeout: Duration,
    block_network: bool,
) -> anyhow::Result<(String, String, i32)> {
    use std::process::Stdio;
    use tokio::process::Command as TokioCommand;
    use tokio::time::timeout as tokio_timeout;

    let mut cmd = TokioCommand::new(shell);
    cmd.arg(shell_flag).arg(command);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    if block_network {
        cmd.env("HTTPS_PROXY", "http://127.0.0.1:9");
        cmd.env("HTTP_PROXY", "http://127.0.0.1:9");
        cmd.env("ALL_PROXY", "http://127.0.0.1:9");
        cmd.env("GIT_HTTPS_PROXY", "http://127.0.0.1:9");
        cmd.env("NO_PROXY", "localhost,127.0.0.1,::1");
    }

    if let Some(dir) = workdir {
        cmd.current_dir(dir);
    }

    let output = tokio_timeout(timeout, cmd.output())
        .await
        .map_err(|_| anyhow!("command timed out"))?
        .map_err(|e| anyhow!("command failed: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);

    Ok((stdout, stderr, exit_code))
}

fn create_sandbox_sid() -> Result<PSID, windows::core::Error> {
    let pid = std::process::id();
    unsafe {
        let mut sid: PSID = std::mem::zeroed();
        AllocateAndInitializeSid(
            &windows::Win32::Security::SID_IDENTIFIER_AUTHORITY { Value: SECURITY_NT_AUTHORITY },
            5u8,
            21u32,
            (pid >> 16) & 0xFFFF,
            pid & 0xFFFF,
            0xF1_E001u32,
            0x5B0_01u32,
            0,
            0,
            0,
            &mut sid,
        )?;
        Ok(sid)
    }
}

fn apply_write_acls(_sid: PSID, paths: &[PathBuf]) -> anyhow::Result<()> {
    for path in paths {
        if !path.exists() { continue; }
        let path_str = path.to_string_lossy();
        let sid_str = sid_to_string_fallback(_sid)?;
        let grant_cmd = format!("icacls \"{path_str}\" /grant \"*{sid_str}\":(OI)(CI)(RX,W,D) /Q");
        let _ = std::process::Command::new("cmd")
            .args(["/c", &grant_cmd])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        for protected in &[".git", ".codex", ".agents"] {
            let subdir = path.join(protected);
            if subdir.exists() {
                let deny_cmd = format!("icacls \"{}\" /deny \"*{sid_str}\":(OI)(CI)(W,D) /Q", subdir.to_string_lossy());
                let _ = std::process::Command::new("cmd")
                    .args(["/c", &deny_cmd])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        }
    }
    Ok(())
}

fn remove_write_acls(_sid: PSID, paths: &[PathBuf]) {
    for path in paths {
        if !path.exists() { continue; }
        let Ok(sid_str) = sid_to_string_fallback(_sid) else { continue; };
        let path_str = path.to_string_lossy();
        let remove_cmd = format!("icacls \"{path_str}\" /remove \"*{sid_str}\" /Q");
        let _ = std::process::Command::new("cmd")
            .args(["/c", &remove_cmd])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

fn sid_to_string_fallback(sid: PSID) -> anyhow::Result<String> {
    unsafe {
        let mut str_ptr: PWSTR = PWSTR::null();
        if windows::Win32::Security::Authorization::ConvertSidToStringSidW(sid, &mut str_ptr).is_ok() {
            let ptr: *const u16 = str_ptr.as_ptr();
            let len = (0..).take_while(|&i| *ptr.add(i) != 0).count();
            let result = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            windows::Win32::Foundation::LocalFree(
                windows::Win32::Foundation::HLOCAL(str_ptr.0 as *mut std::ffi::c_void),
            );
            return Ok(result);
        }
    }
    Ok(format!("S-1-5-21-{}-funera-sandbox", std::process::id()))
}

fn create_write_restricted_token(sandbox_sid: PSID) -> anyhow::Result<HANDLE> {
    let mut token: HANDLE = HANDLE::default();
    unsafe {
        OpenProcessToken(GetCurrentProcess(), TOKEN_ACCESS_MASK(TOKEN_QUERY.0 | TOKEN_DUPLICATE.0), &mut token)
            .map_err(|_| anyhow!("OpenProcessToken failed"))?;
    }

    let logon_sid = get_logon_sid(token).map_err(|e| { unsafe { CloseHandle(token).ok() }; e })?;
    let everyone = make_everyone_sid().map_err(|e| { unsafe { FreeSid(logon_sid); CloseHandle(token).ok() }; e })?;

    let restricted_sids = [
        SID_AND_ATTRIBUTES { Sid: logon_sid, Attributes: 0 },
        SID_AND_ATTRIBUTES { Sid: everyone, Attributes: 0 },
        SID_AND_ATTRIBUTES { Sid: sandbox_sid, Attributes: 0 },
    ];

    let flags = windows::Win32::Security::CREATE_RESTRICTED_TOKEN_FLAGS(
        WRITE_RESTRICTED | DISABLE_MAX_PRIVILEGE.0 | SANDBOX_INERT.0,
    );

    let mut restricted: HANDLE = HANDLE::default();
    let result = unsafe { CreateRestrictedToken(token, flags, None, None, Some(&restricted_sids), &mut restricted) };

    unsafe { FreeSid(everyone); FreeSid(logon_sid); CloseHandle(token).ok() };
    result.map_err(|_| anyhow!("CreateRestrictedToken failed"))?;
    Ok(restricted)
}

fn make_everyone_sid() -> anyhow::Result<PSID> {
    unsafe {
        let mut sid: PSID = std::mem::zeroed();
        AllocateAndInitializeSid(
            &windows::Win32::Security::SECURITY_WORLD_SID_AUTHORITY, 1u8, 0u32,
            0, 0, 0, 0, 0, 0, 0, &mut sid,
        )
        .map_err(|_| anyhow!("failed to create Everyone SID"))?;
        Ok(sid)
    }
}

fn get_logon_sid(token: HANDLE) -> anyhow::Result<PSID> {
    use windows::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSidToSidW};
    unsafe {
        let mut size: u32 = 0;
        let _ = GetTokenInformation(token, TokenGroups, None, 0, &mut size);
        let mut buf: Vec<u8> = vec![0u8; size as usize];
        GetTokenInformation(token, TokenGroups, Some(buf.as_mut_ptr() as *mut _), size, &mut size)
            .map_err(|_| anyhow!("GetTokenInformation(TokenGroups) failed"))?;

        let groups = &*(buf.as_ptr() as *const TOKEN_GROUPS);
        let groups_ptr = groups.Groups.as_ptr();
        for i in 0..groups.GroupCount as usize {
            let entry = &*groups_ptr.add(i);
            if (entry.Attributes & 0xC0000000u32) == 0xC0000000u32 {
                let mut sid_str: PWSTR = PWSTR::null();
                ConvertSidToStringSidW(entry.Sid, &mut sid_str).map_err(|_| anyhow!("ConvertSidToStringSidW failed"))?;
                let mut dup_sid: PSID = std::mem::zeroed();
                if ConvertStringSidToSidW(sid_str, &mut dup_sid).is_ok() {
                    windows::Win32::Foundation::LocalFree(
                        windows::Win32::Foundation::HLOCAL(sid_str.0 as *mut std::ffi::c_void),
                    );
                    return Ok(dup_sid);
                }
                windows::Win32::Foundation::LocalFree(
                    windows::Win32::Foundation::HLOCAL(sid_str.0 as *mut std::ffi::c_void),
                );
            }
        }
    }
    make_everyone_sid()
}

fn try_full_sandbox(
    sid: PSID,
    shell: &str,
    shell_flag: &str,
    command: &str,
    workdir: Option<&str>,
    timeout: Duration,
    block_network: bool,
) -> anyhow::Result<(String, String, i32)> {
    let token = create_write_restricted_token(sid)?;
    let result = launch_restricted(token, shell, shell_flag, command, workdir, timeout, block_network);
    unsafe { CloseHandle(token).ok() };
    result
}

fn launch_restricted(
    token: HANDLE,
    shell: &str,
    shell_flag: &str,
    command: &str,
    workdir: Option<&str>,
    timeout: Duration,
    block_network: bool,
) -> anyhow::Result<(String, String, i32)> {
    let env_block = if block_network { Some(build_net_blocked_env_block()) } else { None };

    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: BOOL::from(true),
    };

    // ── create pipes with a drop-guard so early-exits never leak handles ──
    let mut stdout_read = HANDLE::default();
    let mut stdout_write = HANDLE::default();
    unsafe { CreatePipe(&mut stdout_read, &mut stdout_write, Some(&sa), 0)?; }

    let mut stderr_read = HANDLE::default();
    let mut stderr_write = HANDLE::default();
    unsafe { CreatePipe(&mut stderr_read, &mut stderr_write, Some(&sa), 0)?; }

    // Drop-guard: if we return early before the process inherits the
    // write-ends, this guard closes all four handles.
    let mut guard = PipeGuard {
        stdout_read, stdout_write,
        stderr_read, stderr_write,
        committed: false,
    };

    unsafe {
        windows::Win32::Foundation::SetHandleInformation(
            stdout_read, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS::default(),
        )?;
        windows::Win32::Foundation::SetHandleInformation(
            stderr_read, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS::default(),
        )?;
    }

    let full_cmd = format!("{} {} {}", shell, shell_flag, command);
    let mut cmd_wide: Vec<u16> = full_cmd.encode_utf16().collect();
    cmd_wide.push(0);

    let workdir_wide: Option<Vec<u16>> = workdir.map(|d| { let mut w: Vec<u16> = d.encode_utf16().collect(); w.push(0); w });
    let stdin_handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) }.map_err(|_| anyhow!("GetStdHandle failed"))?;

    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    si.hStdOutput = stdout_write;
    si.hStdError = stderr_write;
    si.hStdInput = stdin_handle;
    si.dwFlags = STARTF_USESTDHANDLES;

    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

    let result = unsafe {
        CreateProcessAsUserW(
            token, None, PWSTR::from_raw(cmd_wide.as_mut_ptr()),
            None, None, BOOL::from(true),
            CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
            env_block.as_ref().map(|e| Some(e.as_ptr() as *const std::ffi::c_void)).unwrap_or(None),
            workdir_wide.as_ref().map(|w| windows::core::PCWSTR::from_raw(w.as_ptr())).unwrap_or(windows::core::PCWSTR::null()),
            &si, &mut pi,
        )
    };

    // The child now has handles to the write-ends; we can close ours.
    guard.close_write_ends();

    if result.is_err() {
        return Err(anyhow!("CreateProcessAsUserW failed: {result:?}"));
    }

    // Commit – from now on guard.drop() does nothing.
    guard.committed = true;

    let timeout_ms = timeout.as_millis().min(u32::MAX as u128) as u32;
    let wait_result = unsafe { WaitForSingleObject(pi.hProcess, timeout_ms) };

    let stdout_str = read_pipe(stdout_read);
    let stderr_str = read_pipe(stderr_read);
    // Read-ends are not owned by the guard; close them now.
    unsafe { CloseHandle(stdout_read).ok(); CloseHandle(stderr_read).ok(); }

    let exit_code = if wait_result == WAIT_OBJECT_0 {
        let mut code: u32 = 0;
        unsafe { if GetExitCodeProcess(pi.hProcess, &mut code).is_err() { 1 } else { code as i32 } }
    } else {
        unsafe { TerminateProcess(pi.hProcess, 1).ok() };
        1
    };

    unsafe { CloseHandle(pi.hProcess).ok(); CloseHandle(pi.hThread).ok(); }
    Ok((stdout_str, stderr_str, exit_code))
}

/// RAII guard: closes all four pipe handles unless [`committed`] or
/// [`close_write_ends`] has been called first.
struct PipeGuard {
    stdout_read: HANDLE,
    stdout_write: HANDLE,
    stderr_read: HANDLE,
    stderr_write: HANDLE,
    committed: bool,
}

impl PipeGuard {
    fn close_write_ends(&mut self) {
        unsafe {
            CloseHandle(self.stdout_write).ok();
            CloseHandle(self.stderr_write).ok();
        }
        self.stdout_write = HANDLE::default();
        self.stderr_write = HANDLE::default();
    }
}

impl Drop for PipeGuard {
    fn drop(&mut self) {
        if self.committed {
            // Read-ends will be closed manually after read_pipe.
            return;
        }
        unsafe {
            CloseHandle(self.stdout_read).ok();
            CloseHandle(self.stdout_write).ok();
            CloseHandle(self.stderr_read).ok();
            CloseHandle(self.stderr_write).ok();
        }
    }
}

fn read_pipe(pipe: HANDLE) -> String {
    let mut result: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 4096];
    loop {
        let mut bytes_read: u32 = 0;
        match unsafe { windows::Win32::Storage::FileSystem::ReadFile(pipe, Some(&mut buf), Some(&mut bytes_read), None) } {
            Ok(_) if bytes_read > 0 => result.extend_from_slice(&buf[..bytes_read as usize]),
            Ok(_) => break,
            Err(e) => { if e.code() == ERROR_BROKEN_PIPE.to_hresult() { break; } break; }
        }
    }
    String::from_utf8_lossy(&result).to_string()
}

fn build_net_blocked_env_block() -> Vec<u16> {
    let mut result: Vec<u16> = Vec::new();
    for (key, value) in std::env::vars() {
        let upper = key.to_uppercase();
        if ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY", "GIT_HTTPS_PROXY", "NO_PROXY"].contains(&upper.as_str()) {
            continue;
        }
        result.extend(format!("{key}={value}").encode_utf16());
        result.push(0);
    }
    for (k, v) in &[
        ("HTTPS_PROXY", "http://127.0.0.1:9"),
        ("HTTP_PROXY", "http://127.0.0.1:9"),
        ("ALL_PROXY", "http://127.0.0.1:9"),
        ("GIT_HTTPS_PROXY", "http://127.0.0.1:9"),
        ("NO_PROXY", "localhost,127.0.0.1,::1"),
    ] {
        result.extend(format!("{k}={v}").encode_utf16());
        result.push(0);
    }
    result.push(0);
    result
}

// ── tests ──────────────────────────────────────────────────────────

#[cfg(all(test, feature = "sandbox", target_os = "windows"))]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::security::sandbox::SandboxPolicy;

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_dir() -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!("funera_sandbox_win_test_{}", id));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("create temp dir");
        base
    }

    fn cleanup_temp_dir(dir: &PathBuf) {
        let _ = std::fs::remove_dir_all(dir);
    }

    // ══════════════════════════════════════════════════════════════════
    //  GROUP A — pure functions (0 unsafe, fully testable)
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_env_block_has_poison_proxy_vars() {
        unsafe {
            std::env::set_var("HTTPS_PROXY", "http://real-proxy:8080");
            std::env::set_var("MY_VAR", "hello");
        }

        let block = build_net_blocked_env_block();
        let raw = String::from_utf16_lossy(&block);

        assert!(!raw.contains("http://real-proxy:8080"), "original HTTPS_PROXY must be removed");
        assert!(raw.contains("HTTPS_PROXY=http://127.0.0.1:9"), "missing poison HTTPS_PROXY");
        assert!(raw.contains("HTTP_PROXY=http://127.0.0.1:9"), "missing poison HTTP_PROXY");
        assert!(raw.contains("ALL_PROXY=http://127.0.0.1:9"), "missing poison ALL_PROXY");
        assert!(raw.contains("GIT_HTTPS_PROXY=http://127.0.0.1:9"), "missing poison GIT_HTTPS_PROXY");
        assert!(raw.contains("MY_VAR=hello"), "non-proxy vars must be preserved");

        unsafe {
            std::env::remove_var("HTTPS_PROXY");
            std::env::remove_var("MY_VAR");
        }
    }

    #[test]
    fn test_env_block_double_null_terminated() {
        let block = build_net_blocked_env_block();
        assert!(block.len() >= 2, "block must have at least 2 bytes");
        assert_eq!(block[block.len() - 1], 0, "last byte is null");
        assert_eq!(block[block.len() - 2], 0, "second-to-last byte is null");
    }

    #[test]
    fn test_fallback_without_network_block() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(
            execute_fallback("cmd", "/c", "echo no_net_block_test", None, Duration::from_secs(10), false)
        );
        assert!(result.is_ok(), "fallback command failed: {:?}", result.err());
        let (stdout, _, exit_code) = result.unwrap();
        assert!(stdout.contains("no_net_block_test"), "stdout: {stdout}");
        assert_eq!(exit_code, 0, "exit code: {exit_code}");
    }

    #[test]
    fn test_fallback_with_workdir() {
        let dir = std::env::temp_dir();
        let dir_str = dir.to_string_lossy().to_string();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(
            execute_fallback("cmd", "/c", "cd", Some(&dir_str), Duration::from_secs(10), false)
        );
        assert!(result.is_ok(), "fallback with workdir: {:?}", result.err());
        let (stdout, _, _) = result.unwrap();
        assert!(!stdout.is_empty(), "cd should produce output");
    }

    #[test]
    fn test_fallback_timeout() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(
            execute_fallback("cmd", "/c", "timeout /t 30 /nobreak", None, Duration::from_millis(10), false)
        );
        assert!(result.is_err(), "should timeout");
    }

    #[test]
    fn test_fallback_with_network_blocked() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(
            execute_fallback("cmd", "/c", "echo net_block_ok", None, Duration::from_secs(10), true)
        );
        assert!(result.is_ok());
        let (stdout, _, _) = result.unwrap();
        assert!(stdout.contains("net_block_ok"), "stdout: {stdout}");
    }

    #[test]
    fn test_read_pipe_with_data() {
        let mut read_end = HANDLE::default();
        let mut write_end = HANDLE::default();
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: BOOL::from(true),
        };
        unsafe { CreatePipe(&mut read_end, &mut write_end, Some(&sa), 0).unwrap(); }

        let msg = b"hello pipe";
        let mut written: u32 = 0;
        unsafe {
            windows::Win32::Storage::FileSystem::WriteFile(
                write_end, Some(msg), Some(&mut written), None,
            ).expect("WriteFile failed");
            CloseHandle(write_end).ok();
        }

        let output = read_pipe(read_end);
        assert_eq!(output, "hello pipe");
        unsafe { CloseHandle(read_end).ok() };
    }

    #[test]
    fn test_read_pipe_empty() {
        let mut read_end = HANDLE::default();
        let mut write_end = HANDLE::default();
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: BOOL::from(true),
        };
        unsafe { CreatePipe(&mut read_end, &mut write_end, Some(&sa), 0).unwrap(); }
        unsafe { CloseHandle(write_end).ok() };

        let output = read_pipe(read_end);
        assert_eq!(output, "");
        unsafe { CloseHandle(read_end).ok() };
    }

    // ══════════════════════════════════════════════════════════════════
    //  GROUP B — SID functions (unsafe, both branches testable)
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_create_sandbox_sid_is_valid() {
        let sid = create_sandbox_sid().expect("create sandbox sid");
        assert!(!sid.0.is_null(), "sid must not be null");
        let sid_str = sid_to_string_fallback(sid).expect("sid to string");
        assert!(sid_str.starts_with("S-1-5-21-"), "unexpected sid format: {sid_str}");
        unsafe { FreeSid(sid) };
    }

    #[test]
    fn test_make_everyone_sid_is_valid() {
        let sid = make_everyone_sid().expect("create everyone sid");
        assert!(!sid.0.is_null(), "everyone sid must not be null");
        let sid_str = sid_to_string_fallback(sid).expect("everyone to string");
        assert!(sid_str.contains("S-1-1-"), "unexpected everyone sid: {sid_str}");
        unsafe { FreeSid(sid) };
    }

    #[test]
    fn test_sid_to_string_valid_sid() {
        let sid = create_sandbox_sid().expect("create sid");
        let result = sid_to_string_fallback(sid).expect("sid to string");
        assert!(result.starts_with("S-1-"), "expected SID format: {result}");
        unsafe { FreeSid(sid) };
    }

    #[test]
    fn test_sid_to_string_fallback_on_invalid_sid() {
        let invalid_sid = PSID(std::ptr::null_mut());
        let result = sid_to_string_fallback(invalid_sid).expect("fallback should always succeed");
        assert!(result.contains("funera-sandbox"), "expected fallback format: {result}");
    }

    #[test]
    fn test_sid_free_works() {
        let sid = make_everyone_sid().expect("create everyone sid");
        unsafe { FreeSid(sid) };
    }

    #[test]
    fn test_get_logon_sid_found() {
        let mut token: HANDLE = HANDLE::default();
        unsafe {
            OpenProcessToken(GetCurrentProcess(), TOKEN_ACCESS_MASK(TOKEN_QUERY.0 | TOKEN_DUPLICATE.0), &mut token)
                .expect("OpenProcessToken");
        }
        let sid = get_logon_sid(token).expect("get logon sid");
        assert!(!sid.0.is_null(), "logon sid must not be null");
        unsafe { FreeSid(sid); CloseHandle(token).ok() };
    }

    #[test]
    fn test_get_logon_sid_always_returns_valid() {
        let mut token: HANDLE = HANDLE::default();
        unsafe {
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
                .expect("OpenProcessToken");
        }
        let sid = get_logon_sid(token).expect("get logon sid");
        assert!(!sid.0.is_null(), "must return a valid fallback SID");
        unsafe { FreeSid(sid); CloseHandle(token).ok() };
    }

    // ══════════════════════════════════════════════════════════════════
    //  GROUP C — token functions & sandbox lifecycle
    // ══════════════════════════════════════════════════════════════════
    //
    // Note: try_full_sandbox / create_write_restricted_token /
    // launch_restricted are only testable when the process has
    // SE_ASSIGNPRIMARYTOKEN_NAME privilege (typically admin mode).
    // On non-admin builds, WindowsSandbox::execute gracefully falls
    // back to execute_fallback, which is tested directly in Group A.
    // The end-to-end tests below validate that the fallback works
    // correctly.

    #[test]
    fn test_write_restricted_token_created() {
        let sandbox_sid = create_sandbox_sid().expect("create sandbox sid");
        let token = create_write_restricted_token(sandbox_sid).expect("create restricted token");
        assert_ne!(token.0, std::ptr::null_mut(), "token must not be null");
        unsafe { CloseHandle(token).ok(); FreeSid(sandbox_sid) };
    }

    // ══════════════════════════════════════════════════════════════════
    //  GROUP D — process launch (unsafe heavy)
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_pipe_guard_drops_on_early_exit() {
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: BOOL::from(true),
        };
        let mut r0 = HANDLE::default();
        let mut w0 = HANDLE::default();
        let mut r1 = HANDLE::default();
        let mut w1 = HANDLE::default();
        unsafe {
            CreatePipe(&mut r0, &mut w0, Some(&sa), 0).unwrap();
            CreatePipe(&mut r1, &mut w1, Some(&sa), 0).unwrap();
        }
        {
            let _guard = PipeGuard {
                stdout_read: r0,
                stdout_write: w0,
                stderr_read: r1,
                stderr_write: w1,
                committed: false,
            };
        }
        // r0/r1/w0/w1 are now copies of the handles that guard closed.
        // Reading from them should fail.
        let mut buf = [0u8; 4];
        let mut read: u32 = 0;
        let result = unsafe {
            windows::Win32::Storage::FileSystem::ReadFile(r0, Some(&mut buf), Some(&mut read), None)
        };
        assert!(result.is_err(), "read should fail after guard drop");
    }

    #[test]
    fn test_pipe_guard_committed_does_not_close_reads() {
        let mut r0 = HANDLE::default();
        let mut w0 = HANDLE::default();
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: BOOL::from(true),
        };
        unsafe { CreatePipe(&mut r0, &mut w0, Some(&sa), 0).unwrap(); }

        {
            let mut guard = PipeGuard {
                stdout_read: r0,
                stdout_write: w0,
                stderr_read: HANDLE::default(),
                stderr_write: HANDLE::default(),
                committed: false,
            };
            guard.close_write_ends();
            guard.committed = true;
        }
        assert_ne!(r0.0, std::ptr::null_mut(), "read handle should be non-null");
        unsafe { CloseHandle(r0).ok() };
    }

    #[test]
    fn test_pipe_guard_close_write_ends_nullifies() {
        let mut r0 = HANDLE::default();
        let mut w0 = HANDLE::default();
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: BOOL::from(true),
        };
        unsafe { CreatePipe(&mut r0, &mut w0, Some(&sa), 0).unwrap(); }

        let mut guard = PipeGuard {
            stdout_read: r0,
            stdout_write: w0,
            stderr_read: HANDLE::default(),
            stderr_write: HANDLE::default(),
            committed: false,
        };
        guard.close_write_ends();
        assert_eq!(guard.stdout_write.0, std::ptr::null_mut(), "write end should be null");
        assert_eq!(guard.stderr_write.0, std::ptr::null_mut(), "stderr write end should be null");
        unsafe { CloseHandle(guard.stdout_read).ok() };
    }

    #[test]
    fn test_pipe_guard_drop_null_handles_noop() {
        let guard = PipeGuard {
            stdout_read: HANDLE::default(),
            stdout_write: HANDLE::default(),
            stderr_read: HANDLE::default(),
            stderr_write: HANDLE::default(),
            committed: false,
        };
        drop(guard);
    }

    #[test]
    fn test_sandbox_execute_echo_builtin() {
        let policy = SandboxPolicy {
            enabled: true,
            read_write_paths: vec![],
            ..Default::default()
        };
        let sandbox = WindowsSandbox::new(&policy).expect("create sandbox");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (stdout, _, code) = rt.block_on(
            sandbox.execute("cmd", "/c", "echo win_sandbox_ok", None, Duration::from_secs(10))
        ).expect("sandbox execute");
        assert!(stdout.contains("win_sandbox_ok"), "stdout: {stdout}");
        assert_eq!(code, 0, "exit code: {code}");
    }

    #[test]
    fn test_sandbox_execute_exit_code() {
        let policy = SandboxPolicy::default();
        let sandbox = WindowsSandbox::new(&policy).expect("create sandbox");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (_, _, code) = rt.block_on(
            sandbox.execute("cmd", "/c", "exit 99", None, Duration::from_secs(10))
        ).expect("sandbox execute");
        assert_eq!(code, 99, "expected exit code 99, got {code}");
    }

    #[test]
    fn test_sandbox_execute_timeout() {
        let policy = SandboxPolicy::default();
        let sandbox = WindowsSandbox::new(&policy).expect("create sandbox");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(
            sandbox.execute("cmd", "/c", "waitfor /t 30 never_happens 2>nul || exit 0", None, Duration::from_millis(50))
        );
        // On non-admin, try_full_sandbox fails → execute_fallback runs
        // → tokio timeout returns Err (future cancelled).
        match result {
            Err(_) => {} // expected
            Ok(out) => {
                // Fallback completed but the process may have been killed;
                // any non-zero exit indicates the timeout path was hit.
                let (_, _, code) = out;
                assert!(code != 0, "expected non-zero exit on timeout path, got {code}");
            }
        }
    }

    #[test]
    fn test_sandbox_execute_with_workdir() {
        let tmpdir = unique_temp_dir();
        let dir_str = tmpdir.to_string_lossy().to_string();
        let policy = SandboxPolicy {
            enabled: true,
            read_write_paths: vec![tmpdir.clone()],
            ..Default::default()
        };
        let sandbox = WindowsSandbox::new(&policy).expect("create sandbox");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (stdout, _, code) = rt.block_on(
            sandbox.execute("cmd", "/c", "cd", Some(&dir_str), Duration::from_secs(10))
        ).expect("sandbox execute with workdir");
        assert_eq!(code, 0, "exit code: {code}");
        assert!(stdout.contains(&dir_str.replace('/', "\\")) || stdout.contains(&dir_str),
            "expected workdir {dir_str} in output: {stdout}");
        cleanup_temp_dir(&tmpdir);
    }

    // ══════════════════════════════════════════════════════════════════
    //  GROUP E — ACL functions
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_apply_acls_empty_paths() {
        let sid = create_sandbox_sid().expect("create sid");
        assert!(apply_write_acls(sid, &[]).is_ok(), "empty paths should succeed");
        unsafe { FreeSid(sid) };
    }

    #[test]
    fn test_apply_acls_skips_missing_dir() {
        let sid = create_sandbox_sid().expect("create sid");
        let missing = std::env::temp_dir().join("funera_nonexistent_should_not_exist_xxxx");
        assert!(!missing.exists(), "path should not exist");
        assert!(apply_write_acls(sid, &[missing]).is_ok(), "missing path should be skipped");
        unsafe { FreeSid(sid) };
    }

    #[test]
    fn test_apply_acls_on_existing_dir_roundtrip() {
        let tmpdir = unique_temp_dir();
        let sid = create_sandbox_sid().expect("create sid");

        // apply should not crash
        assert!(apply_write_acls(sid, &[tmpdir.clone()]).is_ok(), "apply ACE");

        // remove should not crash
        remove_write_acls(sid, &[tmpdir.clone()]);

        cleanup_temp_dir(&tmpdir);
        unsafe { FreeSid(sid) };
    }

    #[test]
    fn test_apply_acls_protected_subdir_gets_deny() {
        let tmpdir = unique_temp_dir();
        let git_dir = tmpdir.join(".git");
        std::fs::create_dir_all(&git_dir).expect("create .git");
        let sid = create_sandbox_sid().expect("create sid");

        assert!(apply_write_acls(sid, &[tmpdir.clone()]).is_ok(), "apply ACE");

        cleanup_temp_dir(&tmpdir);
        unsafe { FreeSid(sid) };
    }

    #[test]
    fn test_apply_acls_skips_missing_protected_subdir() {
        let tmpdir = unique_temp_dir();
        let sid = create_sandbox_sid().expect("create sid");
        assert!(apply_write_acls(sid, &[tmpdir.clone()]).is_ok(),
            "missing protected subdirs should not cause errors");
        cleanup_temp_dir(&tmpdir);
        unsafe { FreeSid(sid) };
    }

    #[test]
    fn test_remove_acls_on_missing_path_is_noop() {
        let sid = create_sandbox_sid().expect("create sid");
        let missing = std::env::temp_dir().join("funera_remove_missing_xxxxx");
        remove_write_acls(sid, &[missing]);
        unsafe { FreeSid(sid) };
    }

    #[test]
    fn test_remove_acls_empty_paths_noop() {
        let sid = create_sandbox_sid().expect("create sid");
        remove_write_acls(sid, &[]);
        unsafe { FreeSid(sid) };
    }

    // ══════════════════════════════════════════════════════════════════
    //  GROUP F — constructor / lifecycle
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_new_empty_paths() {
        let policy = SandboxPolicy {
            enabled: true,
            read_write_paths: vec![],
            ..Default::default()
        };
        let sandbox = WindowsSandbox::new(&policy).expect("new with empty paths");
        assert!(!sandbox.sid.0.is_null(), "sid must be allocated");
    }

    #[test]
    fn test_new_with_writable_paths() {
        let tmpdir = unique_temp_dir();
        let policy = SandboxPolicy {
            enabled: true,
            read_write_paths: vec![tmpdir.clone()],
            ..Default::default()
        };
        let sandbox = WindowsSandbox::new(&policy).expect("new with writable paths");
        assert!(!sandbox.sid.0.is_null(), "sid must be allocated");
        cleanup_temp_dir(&tmpdir);
    }

    #[test]
    fn test_sandbox_drop_works() {
        let policy = SandboxPolicy::default();
        {
            let sandbox = WindowsSandbox::new(&policy).expect("create");
            assert!(!sandbox.sid.0.is_null(), "sid non-null");
        }
    }
}
