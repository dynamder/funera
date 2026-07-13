//! Windows sandbox implementation using write-restricted tokens + environment blocking.
//!
//! Based on OpenAI Codex's "unprivileged sandbox" approach:
//! - File-write isolation: Launch child as a different integrity level or
//!   with a write-restricted token if supported.
//! - Network isolation: environment variable poisoning (advisory, but effective
//!   for most developer tools).
//!
//! The sandbox has two tiers:
//! 1. **Full** (Windows 8+): Write-Restricted Token prevents writes outside
//!    allowed paths by using a synthetic SID + ACLs on writable directories.
//! 2. **Network-only** (fallback): Environment variable poisoning blocks
//!    outbound HTTP/HTTPS traffic for proxy-aware tools.
//!
//! When the full sandbox cannot be applied (missing privileges, unsupported
//! Windows version), execution falls back to the normal path with network
//! restrictions only.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::anyhow;
use funera_core::security::sandbox::SandboxPolicy;

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

/// Write-Restricted Token flag (Windows 8+)
const WRITE_RESTRICTED: u32 = 0x0000_0008;
const SECURITY_NT_AUTHORITY: [u8; 6] = [0, 0, 0, 0, 0, 5];

pub struct WindowsSandbox {
    sid: PSID,
    read_write_paths: Vec<PathBuf>,
    block_network: bool,
}

// PSID wraps *mut c_void which is !Send+!Sync.
// Safe: SID is allocated once, accessed immutably, freed on Drop.
unsafe impl Send for WindowsSandbox {}
unsafe impl Sync for WindowsSandbox {}

impl WindowsSandbox {
    pub fn new(policy: &SandboxPolicy) -> anyhow::Result<Self> {
        let sid = create_sandbox_sid()
            .map_err(|e| anyhow!("failed to create sandbox SID: {e}"))?;

        apply_write_acls(sid, &policy.read_write_paths)?;

        Ok(Self {
            sid,
            read_write_paths: policy.read_write_paths.clone(),
            block_network: policy.block_network,
        })
    }

    pub async fn execute(
        &self,
        command: &str,
        workdir: Option<&str>,
        timeout: Duration,
    ) -> anyhow::Result<(String, String, i32)> {
        // Try full sandbox first (sync path — no !Send across await)
        match try_full_sandbox(self.sid, command, workdir, timeout, self.block_network) {
            Ok(result) => return Ok(result),
            Err(_) => {}
        }
        // Fall back to normal execution with network blocking
        execute_fallback(command, workdir, timeout, self.block_network).await
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

// ── fallback execution (normal cmd with network env blocking) ─────

async fn execute_fallback(
    command: &str,
    workdir: Option<&str>,
    timeout: Duration,
    block_network: bool,
) -> anyhow::Result<(String, String, i32)> {
    use std::process::Stdio;
    use tokio::process::Command as TokioCommand;
    use tokio::time::timeout as tokio_timeout;

    let mut cmd = TokioCommand::new("cmd");
    cmd.arg("/c").arg(command);
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

// ── SID ────────────────────────────────────────────────────────────

fn create_sandbox_sid() -> Result<PSID, windows::core::Error> {
    let pid = std::process::id();
    unsafe {
        let mut sid: PSID = std::mem::zeroed();
        AllocateAndInitializeSid(
            &windows::Win32::Security::SID_IDENTIFIER_AUTHORITY {
                Value: SECURITY_NT_AUTHORITY,
            },
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

// ── ACL ────────────────────────────────────────────────────────────

fn apply_write_acls(_sid: PSID, paths: &[PathBuf]) -> anyhow::Result<()> {
    // Use icacls through cmd to set ACLs. This avoids the complex
    // windows crate ACL API while achieving the same result.
    for path in paths {
        if !path.exists() {
            continue;
        }
        let path_str = path.to_string_lossy();
        let sid_str = sid_to_string_fallback(_sid)?;

        // Grant write to sandbox SID
        let grant_cmd = format!(
            "icacls \"{}\" /grant \"*{}\":(OI)(CI)(RX,W,D) /Q",
            path_str, sid_str
        );
        let _ = std::process::Command::new("cmd")
            .args(["/c", &grant_cmd])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        // Deny write to protected subdirs
        for protected in &[".git", ".codex", ".agents"] {
            let subdir = path.join(protected);
            if subdir.exists() {
                let deny_cmd = format!(
                    "icacls \"{}\" /deny \"*{}\":(OI)(CI)(W,D) /Q",
                    subdir.to_string_lossy(),
                    sid_str
                );
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
    // Best-effort cleanup
    for path in paths {
        if !path.exists() {
            continue;
        }
        let Ok(sid_str) = sid_to_string_fallback(_sid) else {
            continue;
        };
        let path_str = path.to_string_lossy();
        let remove_cmd = format!(
            "icacls \"{}\" /remove \"*{}\" /Q",
            path_str, sid_str
        );
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
        if windows::Win32::Security::Authorization::ConvertSidToStringSidW(sid, &mut str_ptr)
            .is_ok()
        {
            let ptr: *const u16 = str_ptr.as_ptr();
            let len = (0..).take_while(|&i| *ptr.add(i) != 0).count();
            let result = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            windows::Win32::Foundation::LocalFree(
                windows::Win32::Foundation::HLOCAL(str_ptr.0 as *mut std::ffi::c_void),
            );
            return Ok(result);
        }
    }
    let pid = std::process::id();
    Ok(format!("S-1-5-21-{pid:x}-funera-sandbox"))
}

// ── token ──────────────────────────────────────────────────────────

fn create_write_restricted_token(sandbox_sid: PSID) -> anyhow::Result<HANDLE> {
    let mut token: HANDLE = HANDLE::default();
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ACCESS_MASK(TOKEN_QUERY.0 | TOKEN_DUPLICATE.0),
            &mut token,
        )
        .map_err(|_| anyhow!("OpenProcessToken failed"))?;
    }

    let logon_sid = get_logon_sid(token).map_err(|e| {
        unsafe { CloseHandle(token).ok() };
        e
    })?;

    let everyone = make_everyone_sid().map_err(|e| {
        unsafe {
            FreeSid(logon_sid);
            CloseHandle(token).ok();
        }
        e
    })?;

    let restricted_sids = [
        SID_AND_ATTRIBUTES { Sid: logon_sid, Attributes: 0 },
        SID_AND_ATTRIBUTES { Sid: everyone, Attributes: 0 },
        SID_AND_ATTRIBUTES { Sid: sandbox_sid, Attributes: 0 },
    ];

    let flags = windows::Win32::Security::CREATE_RESTRICTED_TOKEN_FLAGS(
        WRITE_RESTRICTED | DISABLE_MAX_PRIVILEGE.0 | SANDBOX_INERT.0,
    );

    let mut restricted: HANDLE = HANDLE::default();
    let result = unsafe {
        CreateRestrictedToken(
            token,
            flags,
            None,
            None,
            Some(&restricted_sids),
            &mut restricted,
        )
    };

    unsafe {
        FreeSid(everyone);
        FreeSid(logon_sid);
        CloseHandle(token).ok();
    }

    result.map_err(|_| anyhow!("CreateRestrictedToken failed"))?;
    Ok(restricted)
}

fn make_everyone_sid() -> anyhow::Result<PSID> {
    unsafe {
        let mut sid: PSID = std::mem::zeroed();
        AllocateAndInitializeSid(
            &windows::Win32::Security::SECURITY_WORLD_SID_AUTHORITY,
            1u8,
            0u32,
            0, 0, 0, 0, 0, 0,
            0,
            &mut sid,
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
        GetTokenInformation(
            token,
            TokenGroups,
            Some(buf.as_mut_ptr() as *mut _),
            size,
            &mut size,
        )
        .map_err(|_| anyhow!("GetTokenInformation(TokenGroups) failed"))?;

        let groups = &*(buf.as_ptr() as *const TOKEN_GROUPS);
        let groups_ptr = groups.Groups.as_ptr();
        for i in 0..groups.GroupCount as usize {
            let entry = &*groups_ptr.add(i);
            let attr = entry.Attributes;
            if (attr & 0xC0000000u32) == 0xC0000000u32 {
                // Duplicate the SID by string round-trip
                let mut sid_str: PWSTR = PWSTR::null();
                ConvertSidToStringSidW(entry.Sid, &mut sid_str)
                    .map_err(|_| anyhow!("ConvertSidToStringSidW failed"))?;

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

// ── process launch ─────────────────────────────────────────────────

/// Attempt the full sandbox (write-restricted token).
/// This must remain synchronous (no .await) because HANDLE is !Send.
fn try_full_sandbox(
    sid: PSID,
    command: &str,
    workdir: Option<&str>,
    timeout: Duration,
    block_network: bool,
) -> anyhow::Result<(String, String, i32)> {
    let token = create_write_restricted_token(sid)?;
    let result = launch_restricted(token, command, workdir, timeout, block_network);
    unsafe { CloseHandle(token).ok() };
    result
}

fn launch_restricted(
    token: HANDLE,
    command: &str,
    workdir: Option<&str>,
    timeout: Duration,
    block_network: bool,
) -> anyhow::Result<(String, String, i32)> {
    let env_block = if block_network {
        Some(build_net_blocked_env_block())
    } else {
        None
    };

    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: BOOL::from(true),
    };

    let mut stdout_read = HANDLE::default();
    let mut stdout_write = HANDLE::default();
    unsafe { CreatePipe(&mut stdout_read, &mut stdout_write, Some(&sa), 0)? };
    unsafe {
        windows::Win32::Foundation::SetHandleInformation(
            stdout_read,
            HANDLE_FLAG_INHERIT.0,
            HANDLE_FLAGS::default(),
        )?;
    }

    let mut stderr_read = HANDLE::default();
    let mut stderr_write = HANDLE::default();
    unsafe { CreatePipe(&mut stderr_read, &mut stderr_write, Some(&sa), 0)? };
    unsafe {
        windows::Win32::Foundation::SetHandleInformation(
            stderr_read,
            HANDLE_FLAG_INHERIT.0,
            HANDLE_FLAGS::default(),
        )?;
    }

    let full_cmd = format!("cmd /c {}", command);
    let mut cmd_wide: Vec<u16> = full_cmd.encode_utf16().collect();
    cmd_wide.push(0);

    let workdir_wide: Option<Vec<u16>> = workdir.map(|d| {
        let mut w: Vec<u16> = d.encode_utf16().collect();
        w.push(0);
        w
    });

    let stdin_handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) }
        .map_err(|_| anyhow!("GetStdHandle failed"))?;

    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    si.hStdOutput = stdout_write;
    si.hStdError = stderr_write;
    si.hStdInput = stdin_handle;
    si.dwFlags = STARTF_USESTDHANDLES;

    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

    let result = unsafe {
        CreateProcessAsUserW(
            token,
            None,
            PWSTR::from_raw(cmd_wide.as_mut_ptr()),
            None,
            None,
            BOOL::from(true),
            CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
            env_block
                .as_ref()
                .map(|e| Some(e.as_ptr() as *const std::ffi::c_void))
                .unwrap_or(None),
            workdir_wide
                .as_ref()
                .map(|w| windows::core::PCWSTR::from_raw(w.as_ptr()))
                .unwrap_or(windows::core::PCWSTR::null()),
            &si,
            &mut pi,
        )
    };

    unsafe {
        CloseHandle(stdout_write).ok();
        CloseHandle(stderr_write).ok();
    }

    if result.is_err() {
        unsafe {
            CloseHandle(stdout_read).ok();
            CloseHandle(stderr_read).ok();
        }
        return Err(anyhow!("CreateProcessAsUserW failed: {result:?}"));
    }

    let timeout_ms = timeout.as_millis().min(u32::MAX as u128) as u32;
    let wait_result = unsafe { WaitForSingleObject(pi.hProcess, timeout_ms) };

    let stdout_str = read_pipe(stdout_read);
    let stderr_str = read_pipe(stderr_read);
    unsafe {
        CloseHandle(stdout_read).ok();
        CloseHandle(stderr_read).ok();
    }

    let exit_code = if wait_result == WAIT_OBJECT_0 {
        let mut code: u32 = 0;
        unsafe {
            if GetExitCodeProcess(pi.hProcess, &mut code).is_err() { 1 }
            else { code as i32 }
        }
    } else {
        unsafe { TerminateProcess(pi.hProcess, 1).ok() };
        1
    };

    unsafe {
        CloseHandle(pi.hProcess).ok();
        CloseHandle(pi.hThread).ok();
    }

    Ok((stdout_str, stderr_str, exit_code))
}

fn read_pipe(pipe: HANDLE) -> String {
    let mut result: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 4096];
    loop {
        let mut bytes_read: u32 = 0;
        let ok = unsafe {
            windows::Win32::Storage::FileSystem::ReadFile(
                pipe,
                Some(&mut buf),
                Some(&mut bytes_read),
                None,
            )
        };
        match ok {
            Ok(_) if bytes_read > 0 => result.extend_from_slice(&buf[..bytes_read as usize]),
            Ok(_) => break,
            Err(e) => {
                if e.code() == ERROR_BROKEN_PIPE.to_hresult() { break; }
                break;
            }
        }
    }
    String::from_utf8_lossy(&result).to_string()
}

// ── network-block environment ──────────────────────────────────────

fn build_net_blocked_env_block() -> Vec<u16> {
    let mut result: Vec<u16> = Vec::new();
    for (key, value) in std::env::vars() {
        let upper = key.to_uppercase();
        if ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY", "GIT_HTTPS_PROXY", "NO_PROXY"]
            .contains(&upper.as_str())
        {
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
