//! # Middleware — agent 事件拦截与处理管道
//!
//! 提供对 ReAct 循环**内部**的可插拔拦截机制，作用于 `process_token_stream` 和
//! `handle_turn_finish` 之后、数据写入 session 历史之前。
//!
//! 分为两类：
//!
//! | 类型 | 特征 | 执行方式 | 影响事件流 |
//! |------|------|----------|------------|
//! | [`InspectorMiddleware`] | 只读观察 | `tokio::spawn` 后台并行 | ❌ 不等待，不阻塞 |
//! | [`MutatorMiddleware`] | 可变处理 | 同步顺序执行 | ✅ 可 Pass/Modify/Block |
//!
//! ## 架构位置
//!
//! ```text
//! LLM Stream → process_token_stream → 聚合结果
//!                                         │
//!                          create E events │
//!                                         ▼
//!                              [middleware chain]
//!                           Pass / Modify / Block
//!                                  │
//!                         ┌────────┴────────┐
//!                         ▼                 ▼
//!                    session 历史         event_tx (通知/callback)
//!                    (下一轮用)           (聚合生成 ChatResponse)
//! ```
//!
//! Middleware 直接操作 session 写入前的数据，因此修改会**持久化**到下一轮
//! ReAct 循环的历史上下文中。
//!
//! ## 快速使用 (在 `funera-orchestrate` 层)
//!
//! ```rust,no_run
//! # use funera_core::middleware::{MiddlewareChain, InspectorMiddleware, MutatorMiddleware,
//! #     InspectorError, MutatorAction, ErrorsDisabled};
//! // 1. 定义 Inspector
//! struct Logger;
//! impl InspectorMiddleware<String> for Logger {
//!     fn name(&self) -> &str { "log" }
//!     fn inspect(&self, _: &String) -> Result<(), InspectorError> { Ok(()) }
//! }
//!
//! // 2. 定义 Mutator
//! struct Censor;
//! impl MutatorMiddleware<String> for Censor {
//!     fn name(&self) -> &str { "censor" }
//!     fn process(&self, s: String) -> MutatorAction<String> { MutatorAction::Pass }
//! }
//!
//! // 3. 构建链
//! let chain = MiddlewareChain::<String>::new()
//!     .with_inspectors((Logger,))
//!     .with_mutator(Censor);
//!
//! // 4. 执行（由 react_loop 内部调用）
//! let result = chain.process("hello".into());
//! ```
//!
//! ## Typestate 错误通道
//!
//! 错误通道通过 typestate 管理：默认 `ErrorsDisabled`，调用
//! [`MiddlewareChain::activate_error_channel`] 后变为 `ErrorsEnabled`，编译期防止重复激活。
//!
//! ```rust,no_run
//! # use funera_core::middleware::{MiddlewareChain, ErrorsEnabled, InspectorMiddleware, InspectorError};
//! # struct Insp;
//! # impl InspectorMiddleware<String> for Insp {
//! #     fn name(&self) -> &str { "i" }
//! #     fn inspect(&self, _: &String) -> Result<(), InspectorError> { Ok(()) }
//! # }
//! let chain = MiddlewareChain::<String>::new()
//!     .with_inspector(Insp);
//! let (enabled, error_rx) = chain.activate_error_channel();
//! // enabled 的类型是 MiddlewareChain<String, ErrorsEnabled>
//! // 无法再次调用 activate_error_channel()
//! ```

use std::marker::PhantomData;
use std::sync::Arc;

use serde_json::Value as JsonValue;
use tokio::sync::mpsc;

use crate::chat::message::{MsgVariant, Role};

// ═══════════════════════════════════════════════════════════════
// Inspector — 只读观察，后台并行，不等待
// ═══════════════════════════════════════════════════════════════

/// Inspector 错误的类型别名。
///
/// 任何实现了 `std::error::Error + Send + Sync + 'static` 的类型都可以作为
/// inspector 错误返回。
pub type InspectorError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// 只读检查器——接收 `&Evt`，后台并行执行，不阻塞事件流。
///
/// Inspector 通过 `tokio::spawn` 并发执行，其错误通过异步 error channel 报告，
/// 不会影响 Mutator 阶段的事件处理。
///
/// # 示例
///
/// ```rust,no_run
/// # use funera_core::middleware::{InspectorMiddleware, InspectorError};
/// struct TokenLogger;
/// impl InspectorMiddleware<String> for TokenLogger {
///     fn name(&self) -> &str { "token_logger" }
///     fn inspect(&self, event: &String) -> Result<(), InspectorError> {
///         eprintln!("[inspector] event: {event}");
///         Ok(())
///     }
/// }
/// ```
pub trait InspectorMiddleware<Evt>: Send + Sync {
    /// 返回此 inspector 的唯一标识名称。
    fn name(&self) -> &str;

    /// 检查事件（只读），返回 `Ok(())` 或错误（通过 error channel 报告）。
    ///
    /// Inspect 的返回值**不会**影响事件流——即使返回 `Err`，事件也会继续传递。
    fn inspect(&self, event: &Evt) -> Result<(), InspectorError>;
}

// ═══════════════════════════════════════════════════════════════
// Mutator — 可变处理，顺序执行，可放行/修改/阻止
// ═══════════════════════════════════════════════════════════════

/// Mutator 对事件的处理决策。
pub enum MutatorAction<Evt> {
    /// 不做任何修改，事件原样放行。
    Pass,
    /// 将事件替换为新的值。
    Modify(Evt),
    /// 阻止事件继续传递，并附上原因。
    Block { reason: String },
}

/// 可变处理器——接收 `Evt`，选择放行/修改/阻止。
///
/// 与 [`InspectorMiddleware`] 不同，mutator 在事件流程中**同步顺序执行**，
/// 其返回的 [`MutatorAction`] 直接决定事件是否继续传递。
///
/// # 示例
///
/// ```rust,no_run
/// # use funera_core::middleware::{MutatorMiddleware, MutatorAction};
/// struct Censor;
/// impl MutatorMiddleware<String> for Censor {
///     fn name(&self) -> &str { "censor" }
///     fn process(&self, event: String) -> MutatorAction<String> {
///         if event.contains("bad") {
///             MutatorAction::Modify(event.replace("bad", "***"))
///         } else {
///             MutatorAction::Block { reason: "contains bad word".into() }
///         }
///     }
/// }
/// ```
pub trait MutatorMiddleware<Evt>: Send + Sync {
    /// 返回此 mutator 的唯一标识名称。
    fn name(&self) -> &str;

    /// 处理事件。支持三种决策：
    /// - [`MutatorAction::Pass`]：放行，事件不变
    /// - [`MutatorAction::Modify`]：替换事件
    /// - [`MutatorAction::Block`]：阻止后续传递
    fn process(&self, event: Evt) -> MutatorAction<Evt>;
}

// ═══════════════════════════════════════════════════════════════
// 注册 trait — Bevy 风格 tuple（不提供 blanket impl，避免冲突）
// ═══════════════════════════════════════════════════════════════

/// 将一组 inspector 转换为 `Vec<Arc<dyn InspectorMiddleware>>`。
///
/// 该 trait 为元组 `(A,)` 到 `(A, B, ..., L, M)`（arity 1..=12）实现，
/// 用于 [`MiddlewareChain::with_inspectors`]。
///
/// 单值 inspector 应使用 [`MiddlewareChain::with_inspector`]。
pub trait IntoInspectors<Evt> {
    /// 将 self 转换为 inspector 向量。
    fn into_inspectors(self) -> Vec<Arc<dyn InspectorMiddleware<Evt>>>;
}

/// 将一组 mutator 转换为 `Vec<Arc<dyn MutatorMiddleware>>`。
///
/// 该 trait 为元组 `(A,)` 到 `(A, B, ..., L, M)`（arity 1..=12）实现，
/// 用于 [`MiddlewareChain::with_mutators`]。
///
/// 单值 mutator 应使用 [`MiddlewareChain::with_mutator`]。
pub trait IntoMutators<Evt> {
    /// 将 self 转换为 mutator 向量。
    fn into_mutators(self) -> Vec<Arc<dyn MutatorMiddleware<Evt>>>;
}

// ── tuple impls (arity 1..=12) ───────────────────────────

macro_rules! impl_into_inspectors_tuple {
    ($($T:ident),+) => {
        impl<Evt, $($T: InspectorMiddleware<Evt> + 'static),+> IntoInspectors<Evt>
            for ($($T,)+)
        {
            #[allow(non_snake_case)]
            fn into_inspectors(self) -> Vec<Arc<dyn InspectorMiddleware<Evt>>> {
                let ($($T,)+) = self;
                vec![$(Arc::new($T) as Arc<dyn InspectorMiddleware<Evt>>),+]
            }
        }
    };
}

macro_rules! impl_into_mutators_tuple {
    ($($T:ident),+) => {
        impl<Evt, $($T: MutatorMiddleware<Evt> + 'static),+> IntoMutators<Evt>
            for ($($T,)+)
        {
            #[allow(non_snake_case)]
            fn into_mutators(self) -> Vec<Arc<dyn MutatorMiddleware<Evt>>> {
                let ($($T,)+) = self;
                vec![$(Arc::new($T) as Arc<dyn MutatorMiddleware<Evt>>),+]
            }
        }
    };
}

impl_into_inspectors_tuple!(A);
impl_into_inspectors_tuple!(A, B);
impl_into_inspectors_tuple!(A, B, C);
impl_into_inspectors_tuple!(A, B, C, D);
impl_into_inspectors_tuple!(A, B, C, D, F);
impl_into_inspectors_tuple!(A, B, C, D, F, G);
impl_into_inspectors_tuple!(A, B, C, D, F, G, H);
impl_into_inspectors_tuple!(A, B, C, D, F, G, H, I);
impl_into_inspectors_tuple!(A, B, C, D, F, G, H, I, J);
impl_into_inspectors_tuple!(A, B, C, D, F, G, H, I, J, K);
impl_into_inspectors_tuple!(A, B, C, D, F, G, H, I, J, K, L);
impl_into_inspectors_tuple!(A, B, C, D, F, G, H, I, J, K, L, M);

impl_into_mutators_tuple!(A);
impl_into_mutators_tuple!(A, B);
impl_into_mutators_tuple!(A, B, C);
impl_into_mutators_tuple!(A, B, C, D);
impl_into_mutators_tuple!(A, B, C, D, F);
impl_into_mutators_tuple!(A, B, C, D, F, G);
impl_into_mutators_tuple!(A, B, C, D, F, G, H);
impl_into_mutators_tuple!(A, B, C, D, F, G, H, I);
impl_into_mutators_tuple!(A, B, C, D, F, G, H, I, J);
impl_into_mutators_tuple!(A, B, C, D, F, G, H, I, J, K);
impl_into_mutators_tuple!(A, B, C, D, F, G, H, I, J, K, L);
impl_into_mutators_tuple!(A, B, C, D, F, G, H, I, J, K, L, M);

// ═══════════════════════════════════════════════════════════════
// MiddlewareLayer + MiddlewareChain (typestate)
// ═══════════════════════════════════════════════════════════════

/// 中间件链中的一层。
///
/// 每层可以是 inspector（内部并行）或 mutator（内部顺序）。
pub enum MiddlewareLayer<Evt> {
    /// Inspector 层——此层内的所有 inspector 通过 `tokio::spawn` 并行执行。
    Inspector(Vec<Arc<dyn InspectorMiddleware<Evt>>>),
    /// Mutator 层——此层内的所有 mutator 按注册顺序依次执行。
    Mutator(Vec<Arc<dyn MutatorMiddleware<Evt>>>),
}

// ── Typestate markers ─────────────────────────────────────────

/// 错误通道**尚未**启用的 typestate 标记。
///
/// 在此状态下，调用 [`MiddlewareChain::activate_error_channel`] 可激活
/// 错误通道并转换为 [`ErrorsEnabled`] 状态。
pub struct ErrorsDisabled;

/// 错误通道**已**启用的 typestate 标记。
///
/// 在此状态下，`activate_error_channel` 不可用（编译期保证）。
/// 可以通过 [`MiddlewareChain::error_sender`] 获取已有 sender。
pub struct ErrorsEnabled;

/// 中间件链——按注册顺序逐层执行。
///
/// 链中的每一层可以是 inspector 或 mutator，通过构建方法指定。
/// 参数 `ErrState` 是 typestate 标记，默认 [`ErrorsDisabled`]。
///
/// ## 执行模型
///
/// 1. **Inspector 层**：同一层内的 inspector 通过 `tokio::spawn` 并行执行，
///    不等待其完成。错误通过 error channel 异步报告。
///    *无 tokio 运行时*：降级为同步执行。
/// 2. **Mutator 层**：同一层内的 mutator 按注册顺序依次执行。
///    遇到 [`MutatorAction::Block`] 立即终止整条链。
///
/// ## Typestate 错误通道
///
/// ```rust,no_run
/// # use funera_core::middleware::{MiddlewareChain, ErrorsEnabled, InspectorMiddleware, InspectorError};
/// # struct Insp;
/// # impl InspectorMiddleware<String> for Insp {
/// #     fn name(&self) -> &str { "i" }
/// #     fn inspect(&self, _: &String) -> Result<(), InspectorError> { Ok(()) }
/// # }
/// // 默认状态：ErrorsDisabled
/// let chain = MiddlewareChain::<String>::new()
///     .with_inspector(Insp);
///
/// // 激活错误通道后变为 ErrorsEnabled
/// let (chain, rx) = chain.activate_error_channel();
/// // chain:  MiddlewareChain<String, ErrorsEnabled>
/// ```
pub struct MiddlewareChain<Evt, ErrState = ErrorsDisabled> {
    layers: Vec<MiddlewareLayer<Evt>>,
    error_tx: Option<mpsc::UnboundedSender<(String, InspectorError)>>,
    _err: PhantomData<ErrState>,
}

// ── 仅 ErrorsDisabled：new + activate ─────────────────────────

impl<Evt: Clone + Send + 'static> MiddlewareChain<Evt, ErrorsDisabled> {
    /// 创建一个新的空中间件链（`ErrorsDisabled` 状态）。
    ///
    /// 链初始为空，需要通过 `with_inspector`、`with_mutators` 等方法填充。
    pub fn new() -> Self {
        Self {
            layers: Vec::new(),
            error_tx: None,
            _err: PhantomData,
        }
    }

    /// 启用错误通道并获取 receiver。
    ///
    /// 调用后链的状态变为 [`ErrorsEnabled`]，inspector 执行中的错误将通过
    /// 返回的 receiver 异步送达。此方法**仅可调用一次**（编译期由 typestate 保证）。
    ///
    /// 如果不需要跟踪 inspector 错误，可以忽略返回值中的 receiver。
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// # use funera_core::middleware::{MiddlewareChain, InspectorMiddleware, InspectorError};
    /// # struct Insp;
    /// # impl InspectorMiddleware<String> for Insp {
    /// #     fn name(&self) -> &str { "i" }
    /// #     fn inspect(&self, _: &String) -> Result<(), InspectorError> { Ok(()) }
    /// # }
    /// let chain = MiddlewareChain::<String>::new()
    ///     .with_inspector(Insp);
    /// let (_chain, mut error_rx) = chain.activate_error_channel();
    /// ```
    pub fn activate_error_channel(
        self,
    ) -> (
        MiddlewareChain<Evt, ErrorsEnabled>,
        mpsc::UnboundedReceiver<(String, InspectorError)>,
    ) {
        let (tx, rx) = mpsc::unbounded_channel();
        let chain = MiddlewareChain::<Evt, ErrorsEnabled> {
            layers: self.layers,
            error_tx: Some(tx),
            _err: PhantomData,
        };
        (chain, rx)
    }
}

impl<Evt: Clone + Send + 'static> Default for MiddlewareChain<Evt, ErrorsDisabled> {
    fn default() -> Self {
        Self::new()
    }
}

// ── 构建方法：两种状态均可用 ──────────────────────────────────

impl<Evt: Clone + Send + 'static, S> MiddlewareChain<Evt, S> {
    /// 添加单个 inspector 作为独立的一层。
    ///
    /// 每个 `with_inspector` 调用创建一个新层，与其他层按注册顺序执行。
    /// 同一层内的 inspector 通过 `tokio::spawn` 后台并行。
    pub fn with_inspector(mut self, i: impl InspectorMiddleware<Evt> + 'static) -> Self {
        self.layers
            .push(MiddlewareLayer::Inspector(vec![Arc::new(i)]));
        self
    }

    /// 添加一组 inspector 作为同一层（并行）。
    ///
    /// 接受 Bevy 风格的 tuple `(A, B, C)`，所有 inspector 将在同一层内并行执行。
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// # use funera_core::middleware::{MiddlewareChain, InspectorMiddleware, InspectorError};
    /// # struct A; impl InspectorMiddleware<String> for A {
    /// #     fn name(&self) -> &str { "A" }
    /// #     fn inspect(&self, _: &String) -> Result<(), InspectorError> { Ok(()) }
    /// # }
    /// # struct B; impl InspectorMiddleware<String> for B {
    /// #     fn name(&self) -> &str { "B" }
    /// #     fn inspect(&self, _: &String) -> Result<(), InspectorError> { Ok(()) }
    /// # }
    /// let chain = MiddlewareChain::<String>::new()
    ///     .with_inspectors((A, B));  // A 和 B 并行执行
    /// ```
    pub fn with_inspectors(mut self, i: impl IntoInspectors<Evt>) -> Self {
        let v = i.into_inspectors();
        if !v.is_empty() {
            self.layers.push(MiddlewareLayer::Inspector(v));
        }
        self
    }

    /// 从迭代器批量添加已装箱的 inspector。
    pub fn with_inspectors_from_iter(
        mut self,
        iter: impl IntoIterator<Item = Arc<dyn InspectorMiddleware<Evt>>>,
    ) -> Self {
        let v: Vec<_> = iter.into_iter().collect();
        if !v.is_empty() {
            self.layers.push(MiddlewareLayer::Inspector(v));
        }
        self
    }

    /// 添加单个 mutator 作为独立的一层。
    ///
    /// 每个 `with_mutator` 调用创建一个新层。同一层内的 mutator 按顺序执行。
    pub fn with_mutator(mut self, m: impl MutatorMiddleware<Evt> + 'static) -> Self {
        self.layers.push(MiddlewareLayer::Mutator(vec![Arc::new(m)]));
        self
    }

    /// 添加一组 mutator 作为同一层（顺序执行）。
    ///
    /// 接受 Bevy 风格的 tuple `(A, B, C)`，所有 mutator 按注册顺序依次执行。
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// # use funera_core::middleware::{MiddlewareChain, MutatorMiddleware, MutatorAction};
    /// # struct Censor; impl MutatorMiddleware<String> for Censor {
    /// #     fn name(&self) -> &str { "censor" }
    /// #     fn process(&self, s: String) -> MutatorAction<String> { MutatorAction::Pass }
    /// # }
    /// # struct Blocker; impl MutatorMiddleware<String> for Blocker {
    /// #     fn name(&self) -> &str { "blocker" }
    /// #     fn process(&self, s: String) -> MutatorAction<String> { MutatorAction::Pass }
    /// # }
    /// let chain = MiddlewareChain::<String>::new()
    ///     .with_mutators((Censor, Blocker));  // Censor 先，Blocker 后
    /// ```
    pub fn with_mutators(mut self, m: impl IntoMutators<Evt>) -> Self {
        let v = m.into_mutators();
        if !v.is_empty() {
            self.layers.push(MiddlewareLayer::Mutator(v));
        }
        self
    }

    /// 从迭代器批量添加已装箱的 mutator。
    pub fn with_mutators_from_iter(
        mut self,
        iter: impl IntoIterator<Item = Arc<dyn MutatorMiddleware<Evt>>>,
    ) -> Self {
        let v: Vec<_> = iter.into_iter().collect();
        if !v.is_empty() {
            self.layers.push(MiddlewareLayer::Mutator(v));
        }
        self
    }

    /// 链中是否没有任何 middleware 层。
    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// 返回 middleware 层的数量。
    pub fn len(&self) -> usize {
        self.layers.len()
    }

    /// 按注册顺序逐层执行。
    ///
    /// ## 执行流程
    ///
    /// - **Inspector 层**：`tokio::spawn` 后台并发执行，不等待，错误进入 error channel。
    ///   如果当前没有 tokio 运行时，降级为同步执行。
    /// - **Mutator 层**：同步顺序执行。遇到 `Block` 立即短路，返回 `Err(MiddlewareBlocked)`。
    ///
    /// ## 返回值
    ///
    /// - `Ok(event)` — 经过所有 layer 处理后的最终事件
    /// - `Err(MiddlewareBlocked)` — 被 mutator 阻止
    pub fn process(&self, event: Evt) -> Result<Evt, MiddlewareBlocked> {
        let mut current = event;
        for layer in &self.layers {
            match layer {
                MiddlewareLayer::Inspector(inspectors) => {
                    current = self.run_inspectors(inspectors, current);
                }
                MiddlewareLayer::Mutator(mutators) => {
                    current = self.run_mutators(mutators, current)?;
                }
            }
        }
        Ok(current)
    }

    fn run_inspectors(
        &self,
        inspectors: &[Arc<dyn InspectorMiddleware<Evt>>],
        event: Evt,
    ) -> Evt {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            for insp in inspectors {
                let name = insp.name().to_string();
                let evt = event.clone();
                let tx = self.error_tx.clone();
                let insp = Arc::clone(insp);
                handle.spawn(async move {
                    if let Err(e) = insp.inspect(&evt)
                        && let Some(tx) = tx
                    {
                        let _ = tx.send((name, e));
                    }
                });
            }
        } else {
            for insp in inspectors {
                if let (Some(tx), Err(e)) = (&self.error_tx, insp.inspect(&event)) {
                    let _ = tx.send((insp.name().to_string(), e));
                }
            }
        }
        event
    }

    fn run_mutators(
        &self,
        mutators: &[Arc<dyn MutatorMiddleware<Evt>>],
        event: Evt,
    ) -> Result<Evt, MiddlewareBlocked> {
        let mut current = event;
        for m in mutators {
            match m.process(current.clone()) {
                MutatorAction::Pass => {}
                MutatorAction::Modify(e) => current = e,
                MutatorAction::Block { reason } => {
                    return Err(MiddlewareBlocked {
                        middleware_name: m.name().to_string(),
                        reason,
                    });
                }
            }
        }
        Ok(current)
    }
}

// ── ErrorsEnabled: 暴露 error channel ─────────────────────────

impl<Evt: Clone + Send + 'static> MiddlewareChain<Evt, ErrorsEnabled> {
    /// 返回底层 error channel 的 sender，可用于外部发送错误。
    ///
    /// 返回的 sender 是 `Option`——如果 chain 构造时未激活错误通道则为 `None`。
    pub fn error_sender(&self) -> Option<mpsc::UnboundedSender<(String, InspectorError)>> {
        self.error_tx.clone()
    }
}

// ═══════════════════════════════════════════════════════════════
// MiddlewareBlocked
// ═══════════════════════════════════════════════════════════════

/// 事件被 mutator 阻止时的错误信息。
#[derive(Debug, Clone)]
pub struct MiddlewareBlocked {
    /// 阻止事件的 mutator 名称。
    pub middleware_name: String,
    /// 阻止原因。
    pub reason: String,
}

impl std::fmt::Display for MiddlewareBlocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[middleware:{}] event blocked: {}",
            self.middleware_name, self.reason
        )
    }
}

// ═══════════════════════════════════════════════════════════════
// MiddlewareEvent — ReAct loop 每轮产出的可过滤事件
// ═══════════════════════════════════════════════════════════════

/// 用于 `react_loop` 向上层通知事件的可调用对象。
///
/// 由 `funera-orchestrate` 提供实现（封装 callbacks + event_tx），
/// `react_loop` 在每轮聚合数据后调用此函数发出已过滤的事件。
pub type EventSenderFn<E> = Box<dyn Fn(E) + Send + Sync>;

/// ReAct 循环每轮产出的中间件事件。
///
/// `react_loop` 在 `process_token_stream` 和 `handle_turn_finish` 后
/// 将聚合结果转换为 `MiddlewareEvent`，经过 middleware chain 过滤后：
/// - 发出到上层（`event_tx` + callbacks）
/// - 转换为 `FuneraMessage` 存入 session 历史
///
/// 由 `AgentEvent`（`funera-orchestrate`）实现此 trait。
pub trait MiddlewareEvent: Clone + Send + 'static {
    /// 工具错误类型。
    type Error: std::fmt::Display + Send + Sync + 'static + From<String>;

    /// Factory：assistant 聚合文本回复。
    fn assistant_text(content: String, reasoning: Option<String>) -> Self;

    /// Factory：单个工具调用请求。
    fn tool_call_request(call_id: Arc<str>, name: String, args: JsonValue) -> Self;

    /// Factory：单个工具执行结果。
    fn tool_response(
        call_id: Arc<str>,
        name: String,
        result: Result<String, Self::Error>,
    ) -> Self;

    /// Factory：turn 开始。
    fn turn_start() -> Self;

    /// Factory：turn 结束，携带 finish_reason。
    fn turn_end(finish_reason: Option<String>) -> Self;

    /// Factory：会话结束。
    fn done() -> Self;

    /// 转换为 session 历史消息。
    ///
    /// 返回 `Some((role, variant))` 用于构造 `FuneraMessage`。
    /// 不可转换的事件（如 `TurnStart`、`TurnEnd`、`Done`）返回 `None`。
    fn into_session_message(self) -> Option<(Role, MsgVariant)>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoopInspector;

    impl InspectorMiddleware<String> for NoopInspector {
        fn name(&self) -> &str {
            "noop"
        }
        fn inspect(&self, _event: &String) -> Result<(), InspectorError> {
            Ok(())
        }
    }

    struct UpperMutator;

    impl MutatorMiddleware<String> for UpperMutator {
        fn name(&self) -> &str {
            "upper"
        }
        fn process(&self, event: String) -> MutatorAction<String> {
            MutatorAction::Modify(event.to_uppercase())
        }
    }

    struct BlockMutator;

    impl MutatorMiddleware<String> for BlockMutator {
        fn name(&self) -> &str {
            "blocker"
        }
        fn process(&self, _event: String) -> MutatorAction<String> {
            MutatorAction::Block {
                reason: "blocked".into(),
            }
        }
    }

    struct PassMutator;

    impl MutatorMiddleware<String> for PassMutator {
        fn name(&self) -> &str {
            "pass"
        }
        fn process(&self, _event: String) -> MutatorAction<String> {
            MutatorAction::Pass
        }
    }

    #[test]
    fn new_chain_is_empty() {
        let chain = MiddlewareChain::<String>::new();
        assert!(chain.is_empty());
    }

    #[test]
    fn single_mutator_modify() {
        let chain = MiddlewareChain::<String>::new().with_mutator(UpperMutator);
        let result = chain.process("hello".into()).unwrap();
        assert_eq!(result, "HELLO");
    }

    #[test]
    fn single_mutator_pass() {
        let chain = MiddlewareChain::<String>::new().with_mutator(PassMutator);
        let result = chain.process("hello".into()).unwrap();
        assert_eq!(result, "hello");
    }

    #[test]
    fn single_mutator_block() {
        let chain = MiddlewareChain::<String>::new().with_mutator(BlockMutator);
        let err = chain.process("hello".into()).unwrap_err();
        assert_eq!(err.middleware_name, "blocker");
        assert_eq!(err.reason, "blocked");
    }

    #[test]
    fn pass_then_modify() {
        let chain = MiddlewareChain::<String>::new()
            .with_mutators((PassMutator, UpperMutator));
        let result = chain.process("hello".into()).unwrap();
        assert_eq!(result, "HELLO");
    }

    #[test]
    fn modify_then_block() {
        let chain = MiddlewareChain::<String>::new()
            .with_mutators((UpperMutator, BlockMutator));
        let err = chain.process("hello".into()).unwrap_err();
        assert_eq!(err.middleware_name, "blocker");
    }

    #[test]
    fn tuple_arity_3() {
        let chain = MiddlewareChain::<String>::new()
            .with_mutators((UpperMutator, PassMutator, PassMutator));
        let result = chain.process("hello".into()).unwrap();
        assert_eq!(result, "HELLO");
    }

    #[test]
    fn inspector_without_tokio_is_noop() {
        let chain = MiddlewareChain::<String>::new()
            .with_inspector(NoopInspector)
            .with_mutator(UpperMutator);
        let result = chain.process("hello".into()).unwrap();
        assert_eq!(result, "HELLO");
    }

    #[test]
    fn activate_error_channel_transitions_state() {
        let chain = MiddlewareChain::<String>::new();
        let (_enabled, _rx) = chain.activate_error_channel();
    }

    #[test]
    fn with_inspectors_tuple() {
        let chain = MiddlewareChain::<String>::new()
            .with_inspectors((NoopInspector, NoopInspector));
        assert_eq!(chain.len(), 1);
    }

    #[test]
    fn single_inspector_tuple() {
        let chain = MiddlewareChain::<String>::new()
            .with_inspectors((NoopInspector,));
        assert_eq!(chain.len(), 1);
    }

    #[test]
    fn chain_with_tokio_spawns_inspectors() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            let chain = MiddlewareChain::<String>::new()
                .with_inspector(NoopInspector);
            let result = chain.process("hi".into()).unwrap();
            assert_eq!(result, "hi");
        });
    }
}
