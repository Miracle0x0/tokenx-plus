//! Locale-neutral Subscription diagnostics.
//!
//! Backend code records a stable semantic code, canonical English diagnostic,
//! structured interpolation fields, and typed nested causes. Presentation
//! adapters choose the locale; provider tasks and cache I/O never read process
//! locale state.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubscriptionIssueCode {
    Unexpected,
    FetchPanicked,
    FetchTimeout,
    WorkerDisconnected,
    KeychainMacOnly,
    KeychainLookupFailed,
    CacheClockBeforeEpoch,
    CacheSerialize,
    CachePersist,
    CacheRead,
    CacheMalformed,
    CacheUnsupportedSchema,
    CacheUnsupportedVersion,
    CacheFutureTimestamp,
    ClaudeReadCredentials,
    ClaudeParseCredentials,
    ClaudeNoHomeOrKeychain,
    ClaudeNoCredentials,
    ClaudeRejected,
    ClaudeRequestFailed,
    ClaudeNoOauth,
    ClaudeNoAccessToken,
    CodexNoHome,
    CodexReadAuth,
    CodexParseAuth,
    CodexNoCredentials,
    CodexNoUsableToken,
    CodexRejected,
    CodexRequestFailed,
    CodexAuthPage,
    CodexNoTokens,
    CodexNoAccessToken,
    GrokNoHome,
    GrokReadCredentials,
    GrokParseCredentials,
    GrokAuthNotObject,
    GrokAuthEntriesNone,
    GrokAuthEntriesMultiple,
    GrokMissingUserId,
    GrokMissingPrincipalId,
    GrokFieldEmpty,
    GrokRejected,
    GrokBillingError,
    GrokParseBilling,
    GrokNoData,
    KimiNoHome,
    KimiNoCredential,
    KimiRequestFailed,
    KimiAuthRejected,
    KimiNoAccessToken,
    KimiCredentialExpired,
    KimiEnvRequired,
    MiniMaxSessionExpired,
    MiniMaxRequestFailed,
    MiniMaxApiError,
    MiniMaxNoUsage,
    MiniMaxNoEnv,
    ZaiQuotaFailed,
    ZaiSubscriptionFailed,
    ZaiNoApiKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubscriptionIssue {
    code: SubscriptionIssueCode,
    message: String,
    fields: Vec<(&'static str, String)>,
    cause: Option<Box<Self>>,
}

impl SubscriptionIssue {
    pub(crate) fn new(code: SubscriptionIssueCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            fields: Vec::new(),
            cause: None,
        }
    }

    pub(crate) fn unexpected(error: impl std::fmt::Display) -> Self {
        Self::new(SubscriptionIssueCode::Unexpected, error.to_string())
    }

    pub(crate) fn from_anyhow(error: &anyhow::Error) -> Self {
        error
            .downcast_ref::<Self>()
            .cloned()
            .unwrap_or_else(|| Self::unexpected(format!("{error:#}")))
    }

    pub(crate) fn with_field(mut self, name: &'static str, value: impl ToString) -> Self {
        self.fields.push((name, value.to_string()));
        self
    }

    pub(crate) fn with_cause(mut self, cause: Self) -> Self {
        self.cause = Some(Box::new(cause));
        self
    }

    pub(crate) const fn code(&self) -> SubscriptionIssueCode {
        self.code
    }

    pub(crate) fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find_map(|(field, value)| (*field == name).then_some(value.as_str()))
    }

    pub(crate) fn cause(&self) -> Option<&Self> {
        self.cause.as_deref()
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for SubscriptionIssue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SubscriptionIssue {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause
            .as_deref()
            .map(|cause| cause as &(dyn std::error::Error + 'static))
    }
}
