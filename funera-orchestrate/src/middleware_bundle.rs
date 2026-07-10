//! # MiddlewareBundle — 将 middleware chain 与错误通道打包
//!
//! [`MiddlewareBundle`] 将处于 [`ErrorsEnabled`](funera_core::middleware::ErrorsEnabled)
//! 状态的 [`MiddlewareChain`](funera_core::middleware::MiddlewareChain) 与错误接收端
//! 打包在一起，方便一次性传递给 [`AgentRuntimeBuilder`](crate::runtime::AgentRuntimeBuilder)。
//!
//! ## 使用方式
//!
//! ```rust,no_run
//! # use funera_orchestrate::middleware::*;
//! # use funera_orchestrate::middleware_bundle::MiddlewareBundle;
//! # use funera_orchestrate::AgentEvent;
//! # struct Logger; impl InspectorMiddleware<AgentEvent> for Logger {
//! #     fn name(&self) -> &str { "log" }
//! #     fn inspect(&self, _: &AgentEvent) -> Result<(), InspectorError> { Ok(()) }
//! # }
//! let bundle = MiddlewareBundle::from_chain(
//!     MiddlewareChain::<AgentEvent>::new()
//!         .with_inspectors((Logger,))
//! );
//! // bundle 可直接传入 AgentRuntimeBuilder::with_middleware_bundle()
//! ```

use tokio::sync::mpsc;

use funera_core::middleware::{ErrorsEnabled, InspectorError, MiddlewareChain};

/// 将 middleware chain 与错误接收端打包，供 [`AgentRuntimeBuilder`](crate::runtime::AgentRuntimeBuilder) 使用。
///
/// 创建后可以通过 [`from_chain`](MiddlewareBundle::from_chain) 从 `ErrorsDisabled`
/// 状态的 chain 自动激活错误通道并打包，或通过
/// [`new`](MiddlewareBundle::new) 直接构造。
pub struct MiddlewareBundle<E> {
    /// 已激活错误通道的中间件链。
    pub chain: MiddlewareChain<E, ErrorsEnabled>,
    /// inspector 错误的异步接收端。
    pub error_rx: mpsc::UnboundedReceiver<(String, InspectorError)>,
}

impl<E: Clone + Send + 'static> MiddlewareBundle<E> {
    /// 从 `ErrorsDisabled` 状态的 chain 创建 bundle（自动激活错误通道）。
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// # use funera_orchestrate::middleware::*;
    /// # use funera_orchestrate::middleware_bundle::MiddlewareBundle;
    /// # use funera_orchestrate::AgentEvent;
    /// # struct Logger; impl InspectorMiddleware<AgentEvent> for Logger {
    /// #     fn name(&self) -> &str { "log" }
    /// #     fn inspect(&self, _: &AgentEvent) -> Result<(), InspectorError> { Ok(()) }
    /// # }
    /// let bundle = MiddlewareBundle::from_chain(
    ///     MiddlewareChain::<AgentEvent>::new()
    ///         .with_inspector(Logger)
    /// );
    /// ```
    pub fn from_chain(chain: MiddlewareChain<E>) -> Self {
        let (chain, error_rx) = chain.activate_error_channel();
        Self { chain, error_rx }
    }

    /// 从已激活错误通道的 chain 和 receiver 直接构造。
    ///
    /// 适用于已经手动调用 `activate_error_channel()` 的场景。
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// # use funera_orchestrate::middleware::*;
    /// # use funera_orchestrate::middleware_bundle::MiddlewareBundle;
    /// # use funera_orchestrate::AgentEvent;
    /// # struct Logger; impl InspectorMiddleware<AgentEvent> for Logger {
    /// #     fn name(&self) -> &str { "log" }
    /// #     fn inspect(&self, _: &AgentEvent) -> Result<(), InspectorError> { Ok(()) }
    /// # }
    /// let chain = MiddlewareChain::<AgentEvent>::new()
    ///     .with_inspector(Logger);
    /// let (chain, error_rx) = chain.activate_error_channel();
    /// let bundle = MiddlewareBundle::new(chain, error_rx);
    /// ```
    pub fn new(
        chain: MiddlewareChain<E, ErrorsEnabled>,
        error_rx: mpsc::UnboundedReceiver<(String, InspectorError)>,
    ) -> Self {
        Self { chain, error_rx }
    }
}
