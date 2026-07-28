use crate::claude_diagnostics;
use crate::cli::ClientFlags;
use anyhow::Result;
use tokenx_engine::{ClientId, ClientUniverse};

pub(crate) fn parse_client_id_arg(raw: &str) -> Result<ClientId, String> {
    let normalized = raw.trim().to_ascii_lowercase();
    ClientId::from_str(&normalized).ok_or_else(|| {
        rust_i18n::t!(
            "shared.client.invalid_id",
            raw = raw,
            valid_ids = valid_client_ids()
        )
        .into_owned()
    })
}

pub(crate) fn valid_client_ids() -> String {
    ClientId::iter()
        .map(ClientId::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Resolve the immutable client universe from CLI flags and one typed
/// settings snapshot. The boolean records whether the universe was narrowed
/// by either source; it has no alternate "all clients" sentinel meaning.
pub(crate) fn resolve_client_universe(
    flags: ClientFlags,
    defaults: &[ClientId],
) -> Result<(ClientUniverse, bool)> {
    let clients = if flags.clients.is_empty() {
        defaults
    } else {
        &flags.clients
    };
    if clients.is_empty() {
        return Ok((ClientUniverse::all(), false));
    }
    Ok((ClientUniverse::new(clients.iter().copied())?, true))
}

pub(crate) fn emit_client_diagnostics(diagnostics: &[claude_diagnostics::ClientDiagnostic]) {
    if diagnostics.is_empty() {
        return;
    }

    use colored::Colorize;
    for diagnostic in diagnostics {
        eprintln!(
            "{}",
            format!("  {}: {}", diagnostic.severity, diagnostic.message).yellow()
        );
        eprintln!("{}", format!("  {}", diagnostic.help).bright_black());
    }
}

/// Print the generation's data-health summary to stderr. Data stays on stdout;
/// degraded inputs are warnings, never a failed exit.
pub(crate) fn emit_health_summary(health: &tokenx_engine::input_health::HealthSummary) {
    use colored::Colorize;
    if health.complete() {
        return;
    }
    eprintln!(
        "{}",
        format!(
            "  {}",
            rust_i18n::t!(
                "commands.shared.health_summary",
                degraded = health.degraded_inputs,
                rejected = health.rejected_records(),
                partial = health.partial_inputs(),
                failed = health.failed_inputs()
            )
        )
        .yellow()
    );
}
