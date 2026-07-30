use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use chrono::{Datelike, Duration, NaiveDate};
use clap::{
    error::ErrorKind, Args, Command, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum,
};
use tokenx_engine::{CalendarContext, ClientId, ClientUniverse, DateRange, GroupBy};

use crate::commands::shared::{parse_client_id_arg, resolve_client_universe};
use crate::date_display::format_month_year;
use crate::failure::CliFailure;
use crate::product_paths::ProductPaths;
use crate::settings::Settings;
use crate::theme::ThemeName;
use crate::tui::Tab;

#[derive(Parser, Debug)]
#[command(name = "tokenx")]
#[command(author, version, about = rust_i18n::t!("cli.about.root"))]
pub(crate) struct Cli {
    #[arg(
        long,
        value_enum,
        global = true,
        help = rust_i18n::t!("cli.help.language")
    )]
    pub(crate) language: Option<crate::i18n::Language>,
    #[command(subcommand)]
    pub(crate) command: Option<Commands>,
}

impl Cli {
    /// Parse the process arguments from the current command grammar.
    pub(crate) fn parse_from_env() -> Self {
        let command = localized_command();
        let matches = command
            .try_get_matches_from(std::env::args_os())
            .unwrap_or_else(|error| exit_with_clap_error(error));

        Self::from_arg_matches(&matches).unwrap_or_else(|error| exit_with_clap_error(error))
    }
}

fn localized_command() -> Command {
    let mut command = Cli::command();
    command.build();
    localize_command_tree(command)
}

fn localize_command_tree(mut command: Command) -> Command {
    command = command
        .help_template(rust_i18n::t!("cli.help.template"))
        .subcommand_help_heading(rust_i18n::t!("cli.help.commands_heading").into_owned())
        .mut_args(|arg| {
            let heading = if arg.get_index().is_some() {
                rust_i18n::t!("cli.help.arguments_heading").into_owned()
            } else {
                rust_i18n::t!("cli.help.options_heading").into_owned()
            };

            match arg.get_id().as_ref() {
                "help" => arg
                    .help(rust_i18n::t!("cli.help.print_help"))
                    .long_help(rust_i18n::t!("cli.help.print_help_long"))
                    .help_heading(heading),
                "version" => arg
                    .help(rust_i18n::t!("cli.help.print_version"))
                    .help_heading(heading),
                "subcommand" => arg
                    .help(rust_i18n::t!("cli.help.print_subcommand_help"))
                    .help_heading(heading),
                _ => arg.help_heading(heading),
            }
        });

    if command
        .get_subcommands()
        .any(|subcommand| subcommand.get_name() == "help")
    {
        command = command.mut_subcommand("help", |subcommand| {
            subcommand.about(rust_i18n::t!("cli.help.help_subcommand"))
        });
    }

    command.mut_subcommands(localize_command_tree)
}

fn exit_with_clap_error(error: clap::Error) -> ! {
    let kind = error.kind();
    let output = localize_clap_output(error.render().to_string());
    if matches!(
        kind,
        ErrorKind::DisplayHelp
            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
            | ErrorKind::DisplayVersion
    ) {
        print!("{output}");
    } else {
        eprint!("{output}");
    }
    std::process::exit(error.exit_code());
}

fn localize_clap_output(mut output: String) -> String {
    let replacements = [
        ("error:", rust_i18n::t!("cli.help.generated_error_prefix")),
        ("Usage:", rust_i18n::t!("cli.help.generated_usage_heading")),
        (
            "For more information, try",
            rust_i18n::t!("cli.help.generated_try_help"),
        ),
        (
            "the following required arguments were not provided:",
            rust_i18n::t!("cli.help.generated_required_arguments"),
        ),
        (
            "one or more of the other specified arguments",
            rust_i18n::t!("cli.help.generated_other_arguments"),
        ),
        (
            "some similar arguments exist",
            rust_i18n::t!("cli.help.generated_some_similar_arguments"),
        ),
        (
            "some similar subcommands exist",
            rust_i18n::t!("cli.help.generated_some_similar_subcommands"),
        ),
        (
            "some similar values exist",
            rust_i18n::t!("cli.help.generated_some_similar_values"),
        ),
        (
            "a similar argument exists",
            rust_i18n::t!("cli.help.generated_similar_argument"),
        ),
        (
            "a similar subcommand exists",
            rust_i18n::t!("cli.help.generated_similar_subcommand"),
        ),
        (
            "a similar value exists",
            rust_i18n::t!("cli.help.generated_similar_value"),
        ),
        (
            "cannot be used multiple times",
            rust_i18n::t!("cli.help.generated_cannot_multiple"),
        ),
        (
            "cannot be used with",
            rust_i18n::t!("cli.help.generated_cannot_with"),
        ),
        (
            "equal sign is needed when assigning values to",
            rust_i18n::t!("cli.help.generated_equals"),
        ),
        (
            "a value is required for",
            rust_i18n::t!("cli.help.generated_value_required"),
        ),
        (
            "but none was supplied",
            rust_i18n::t!("cli.help.generated_none_supplied"),
        ),
        (
            "requires a subcommand but one was not provided",
            rust_i18n::t!("cli.help.generated_requires_subcommand"),
        ),
        (
            "unrecognized subcommand",
            rust_i18n::t!("cli.help.generated_unrecognized_subcommand"),
        ),
        (
            "unexpected argument",
            rust_i18n::t!("cli.help.generated_unexpected_argument"),
        ),
        (
            "unexpected value",
            rust_i18n::t!("cli.help.generated_unexpected_value"),
        ),
        (
            "invalid value",
            rust_i18n::t!("cli.help.generated_invalid_value"),
        ),
        (
            "possible values",
            rust_i18n::t!("cli.help.generated_possible_values"),
        ),
        (
            "no more were expected",
            rust_i18n::t!("cli.help.generated_no_more"),
        ),
        (
            "values required by",
            rust_i18n::t!("cli.help.generated_values_required_by"),
        ),
        (
            "values required for",
            rust_i18n::t!("cli.help.generated_values_required_for"),
        ),
        ("the argument", rust_i18n::t!("cli.help.generated_argument")),
        (
            "the subcommand",
            rust_i18n::t!("cli.help.generated_subcommand"),
        ),
        (
            "subcommands",
            rust_i18n::t!("cli.help.generated_subcommands"),
        ),
        ("only ", rust_i18n::t!("cli.help.generated_only")),
        (
            "were provided",
            rust_i18n::t!("cli.help.generated_were_provided"),
        ),
        (
            "was provided",
            rust_i18n::t!("cli.help.generated_was_provided"),
        ),
        ("tip:", rust_i18n::t!("cli.help.generated_tip")),
        (" for ", rust_i18n::t!("cli.help.generated_for")),
        ("found", rust_i18n::t!("cli.help.generated_found")),
    ];

    for (source, target) in replacements {
        output = output.replace(source, target.as_ref());
    }
    output
}

#[derive(Subcommand, Debug)]
pub(crate) enum Commands {
    #[command(about = rust_i18n::t!("cli.about.tui"))]
    Tui(TuiArgs),
    #[command(about = rust_i18n::t!("cli.about.models"))]
    Models(ModelsArgs),
    #[command(about = rust_i18n::t!("cli.about.pricing"))]
    Pricing {
        #[command(subcommand)]
        subcommand: PricingSubcommand,
    },
    #[command(about = rust_i18n::t!("cli.about.cache"))]
    Cache {
        #[command(subcommand)]
        subcommand: CacheSubcommand,
    },
}

#[derive(Args, Debug, Default)]
pub(crate) struct TuiArgs {
    #[arg(long, value_enum, help = rust_i18n::t!("cli.help.tab"))]
    pub(crate) tab: Option<Tab>,
    #[arg(short, long, help = rust_i18n::t!("cli.help.theme"))]
    pub(crate) theme: Option<ThemeName>,
    #[arg(
        short,
        long,
        value_name = "SECONDS",
        value_parser = parse_positive_u64,
        conflicts_with = "no_refresh"
    )]
    pub(crate) refresh: Option<u64>,
    #[arg(long, conflicts_with = "refresh", help = rust_i18n::t!("cli.help.no_refresh"))]
    pub(crate) no_refresh: bool,
    #[arg(long)]
    pub(crate) debug: bool,
    #[command(flatten)]
    pub(crate) input: InputScopeArgs,
    #[command(flatten)]
    pub(crate) date: DateRangeFlags,
}

#[derive(Args, Debug)]
pub(crate) struct ModelsArgs {
    #[arg(long, help = rust_i18n::t!("cli.help.json"))]
    pub(crate) json: bool,
    #[command(flatten)]
    pub(crate) input: InputScopeArgs,
    #[command(flatten)]
    pub(crate) date: DateRangeFlags,
    #[arg(long, help = rust_i18n::t!("cli.help.benchmark"))]
    pub(crate) benchmark: bool,
    #[arg(long, help = rust_i18n::t!("cli.help.no_spinner"))]
    pub(crate) no_spinner: bool,
    #[arg(
        long,
        value_name = "STRATEGY",
        default_value = "model",
        help = rust_i18n::t!("cli.help.group_by")
    )]
    pub(crate) group_by: GroupBy,
}

#[derive(Args, Clone, Debug, Default)]
pub(crate) struct InputScopeArgs {
    #[arg(
        long,
        value_name = "PATH",
        value_parser = parse_home_arg,
        help = rust_i18n::t!("cli.help.home")
    )]
    pub(crate) home: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) clients: ClientFlags,
}

#[derive(Args, Clone, Debug, Default)]
pub(crate) struct ClientFlags {
    /// Canonical client filter. Repeatable or comma-separated.
    #[arg(
        long = "client",
        short = 'c',
        value_parser = parse_client_id_arg,
        value_delimiter = ',',
        action = clap::ArgAction::Append,
        help = rust_i18n::t!("cli.help.client")
    )]
    pub(crate) clients: Vec<ClientId>,
}

#[derive(Args, Clone, Debug, Default)]
pub(crate) struct DateRangeFlags {
    #[arg(
        long,
        conflicts_with_all = ["week", "month", "year", "since", "until"],
        help = rust_i18n::t!("cli.help.today")
    )]
    pub(crate) today: bool,
    #[arg(
        long,
        conflicts_with_all = ["today", "month", "year", "since", "until"],
        help = rust_i18n::t!("cli.help.week")
    )]
    pub(crate) week: bool,
    #[arg(
        long,
        conflicts_with_all = ["today", "week", "year", "since", "until"],
        help = rust_i18n::t!("cli.help.month")
    )]
    pub(crate) month: bool,
    #[arg(
        long,
        value_parser = parse_date_arg,
        conflicts_with_all = ["today", "week", "month", "year"],
        help = rust_i18n::t!("cli.help.since")
    )]
    pub(crate) since: Option<NaiveDate>,
    #[arg(
        long,
        value_parser = parse_date_arg,
        conflicts_with_all = ["today", "week", "month", "year"],
        help = rust_i18n::t!("cli.help.until")
    )]
    pub(crate) until: Option<NaiveDate>,
    #[arg(
        long,
        value_parser = parse_year_arg,
        conflicts_with_all = ["today", "week", "month", "since", "until"],
        help = rust_i18n::t!("cli.help.year")
    )]
    pub(crate) year: Option<i32>,
}

#[derive(Subcommand, Debug)]
pub(crate) enum PricingSubcommand {
    #[command(about = rust_i18n::t!("cli.about.pricing_lookup"))]
    Lookup {
        #[arg(help = rust_i18n::t!("cli.help.model_id"))]
        model_id: String,
        #[arg(long, help = rust_i18n::t!("cli.help.json"))]
        json: bool,
        #[arg(long = "pricing-source", value_enum, help = rust_i18n::t!("cli.help.pricing_source"))]
        pricing_source: Option<PricingSource>,
        #[arg(long, help = rust_i18n::t!("cli.help.no_spinner"))]
        no_spinner: bool,
    },
    #[command(about = rust_i18n::t!("cli.about.pricing_overrides"))]
    Overrides {
        #[arg(long, help = rust_i18n::t!("cli.help.json"))]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum CacheSubcommand {
    #[command(about = rust_i18n::t!("cli.about.cache_warm"))]
    Warm {
        #[command(flatten)]
        input: InputScopeArgs,
    },
    #[command(about = rust_i18n::t!("cli.about.cache_prune"))]
    Prune,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PricingSource {
    Custom,
    Litellm,
    Openrouter,
    #[value(name = "models.dev")]
    ModelsDev,
}

impl PricingSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Custom => "custom",
            Self::Litellm => "litellm",
            Self::Openrouter => "openrouter",
            Self::ModelsDev => "models.dev",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TerminalState {
    pub(crate) stdin: bool,
    pub(crate) stdout: bool,
}

impl TerminalState {
    pub(crate) fn detect() -> Self {
        Self {
            stdin: std::io::stdin().is_terminal(),
            stdout: std::io::stdout().is_terminal(),
        }
    }

    fn interactive(self) -> bool {
        self.stdin && self.stdout
    }
}

#[derive(Debug)]
pub(crate) struct ResolvedInputScope {
    pub(crate) home: PathBuf,
    pub(crate) universe: ClientUniverse,
    pub(crate) restricted: bool,
}

/// One command-start snapshot. Settings are read and validated exactly once,
/// then the immutable input scope and all application policy share this value.
#[derive(Debug)]
pub(crate) struct StartupSnapshot {
    pub(crate) paths: ProductPaths,
    pub(crate) input: ResolvedInputScope,
    pub(crate) settings: Settings,
    pub(crate) calendar: CalendarContext,
    pub(crate) pricing: Arc<tokenx_engine::pricing::ResolvedPricingSnapshot>,
}

#[derive(Debug)]
pub(crate) struct ResolvedDateRange {
    pub(crate) range: DateRange,
    pub(crate) label: Option<String>,
    pub(crate) relative: Option<RelativeDateRange>,
    pub(crate) effective_date: NaiveDate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RelativeDateRange {
    Today,
    LastSevenDays,
    CurrentMonth,
}

impl RelativeDateRange {
    pub(crate) fn resolve(self, current_date: NaiveDate) -> DateRange {
        match self {
            Self::Today => DateRange::bounded(Some(current_date), Some(current_date))
                .expect("a single-day range must be valid"),
            Self::LastSevenDays => {
                DateRange::bounded(Some(current_date - Duration::days(6)), Some(current_date))
                    .expect("the last-seven-days range must be valid")
            }
            Self::CurrentMonth => DateRange::bounded(
                Some(
                    current_date
                        .with_day(1)
                        .expect("every valid date has a first day of its month"),
                ),
                Some(current_date),
            )
            .expect("a current-month range must be valid"),
        }
    }
}

#[derive(Debug)]
pub(crate) struct ModelsPlan {
    pub(crate) json: bool,
    pub(crate) startup: StartupSnapshot,
    pub(crate) date: ResolvedDateRange,
    pub(crate) benchmark: bool,
    pub(crate) no_spinner: bool,
    pub(crate) group_by: GroupBy,
}

#[derive(Debug)]
pub(crate) struct TuiPlan {
    pub(crate) theme: Option<ThemeName>,
    pub(crate) refresh: Option<u64>,
    pub(crate) no_refresh: bool,
    pub(crate) debug: bool,
    pub(crate) startup: StartupSnapshot,
    pub(crate) date: ResolvedDateRange,
    pub(crate) initial_tab: Option<Tab>,
}

#[derive(Debug)]
pub(crate) enum ExecutionPlan {
    Tui(TuiPlan),
    Models(ModelsPlan),
    Pricing {
        paths: ProductPaths,
        subcommand: PricingSubcommand,
    },
    CachePrune(ProductPaths),
    CacheWarm(StartupSnapshot),
}

impl ExecutionPlan {
    pub(crate) fn resolve(cli: Cli, terminal: TerminalState) -> Result<Self, CliFailure> {
        match cli.command.unwrap_or(Commands::Tui(TuiArgs::default())) {
            Commands::Tui(args) => resolve_tui(args, terminal).map(Self::Tui),
            Commands::Models(args) => resolve_models(args).map(Self::Models),
            Commands::Pricing { subcommand } => Ok(Self::Pricing {
                paths: ProductPaths::resolve()?,
                subcommand,
            }),
            Commands::Cache { subcommand } => match subcommand {
                CacheSubcommand::Prune => Ok(Self::CachePrune(ProductPaths::resolve()?)),
                CacheSubcommand::Warm { input } => resolve_startup(input).map(Self::CacheWarm),
            },
        }
    }
}

fn resolve_tui(args: TuiArgs, terminal: TerminalState) -> Result<TuiPlan, CliFailure> {
    if !terminal.interactive() {
        return Err(CliFailure::invalid_message(rust_i18n::t!(
            "cli.error.tui_requires_terminal"
        )));
    }

    let startup = resolve_startup(args.input)?;
    let date = resolve_date(args.date, startup.calendar)?;
    let initial_tab = args.tab;
    if initial_tab == Some(Tab::Subscription) && !startup.settings.subscription.enabled {
        return Err(CliFailure::invalid_message(rust_i18n::t!(
            "cli.error.tui_tab_disabled"
        )));
    }

    Ok(TuiPlan {
        theme: args.theme,
        refresh: args.refresh,
        no_refresh: args.no_refresh,
        debug: args.debug,
        startup,
        date,
        initial_tab,
    })
}

fn resolve_models(args: ModelsArgs) -> Result<ModelsPlan, CliFailure> {
    let startup = resolve_startup(args.input)?;
    let date = resolve_date(args.date, startup.calendar)?;
    Ok(ModelsPlan {
        json: args.json,
        startup,
        date,
        benchmark: args.benchmark,
        no_spinner: args.no_spinner,
        group_by: args.group_by,
    })
}

fn resolve_startup(args: InputScopeArgs) -> Result<StartupSnapshot, CliFailure> {
    // Product state and input discovery are deliberately separate authorities:
    // settings always come from Tokenx's product root, while `--home` changes
    // only the home used to derive built-in client input paths.
    let paths = ProductPaths::resolve()?;
    let settings = Settings::load(&paths)?;
    let calendar = match settings.time_zone {
        Some(calendar) => calendar,
        None => CalendarContext::system().map_err(anyhow::Error::new)?,
    };
    let pricing = Arc::new(
        tokenx_engine::pricing::ResolvedPricingSnapshot::resolve_from(
            &paths.custom_pricing_file(),
            &paths.cache_dir(),
        ),
    );
    let home = args
        .home
        .or_else(dirs::home_dir)
        .ok_or_else(|| CliFailure::invalid_message(rust_i18n::t!("cli.error.home_dir_unknown")))?;
    let (universe, restricted) = resolve_client_universe(args.clients, &settings.default_clients)?;
    Ok(StartupSnapshot {
        paths,
        input: ResolvedInputScope {
            home,
            universe,
            restricted,
        },
        settings,
        calendar,
        pricing,
    })
}

fn resolve_date(
    date: DateRangeFlags,
    calendar: CalendarContext,
) -> Result<ResolvedDateRange, CliFailure> {
    resolve_date_for_date(date, calendar.current_date())
}

pub(crate) fn resolve_date_for_date(
    date: DateRangeFlags,
    current_date: NaiveDate,
) -> Result<ResolvedDateRange, CliFailure> {
    if let (Some(since), Some(until)) = (date.since, date.until) {
        if since > until {
            return Err(CliFailure::invalid_message(rust_i18n::t!(
                "cli.error.reversed_date_range",
                since = since.to_string(),
                until = until.to_string()
            )));
        }
    }

    let (range, label, relative) = if date.today {
        (
            RelativeDateRange::Today.resolve(current_date),
            Some(rust_i18n::t!("cli.date_label.today").into_owned()),
            Some(RelativeDateRange::Today),
        )
    } else if date.week {
        (
            RelativeDateRange::LastSevenDays.resolve(current_date),
            Some(rust_i18n::t!("cli.date_label.last_7_days").into_owned()),
            Some(RelativeDateRange::LastSevenDays),
        )
    } else if date.month {
        (
            RelativeDateRange::CurrentMonth.resolve(current_date),
            Some(format_month_year(current_date)),
            Some(RelativeDateRange::CurrentMonth),
        )
    } else if let Some(year) = date.year {
        (
            DateRange::for_year(year).expect("Clap year parser must validate --year"),
            Some(year.to_string()),
            None,
        )
    } else {
        let mut label_parts = Vec::new();
        if let Some(since) = date.since {
            label_parts.push(rust_i18n::t!("cli.date_label.from", since = since).into_owned());
        }
        if let Some(until) = date.until {
            label_parts.push(rust_i18n::t!("cli.date_label.to", until = until).into_owned());
        }
        (
            DateRange::bounded(date.since, date.until)
                .expect("custom bounds must be ordered before construction"),
            (!label_parts.is_empty()).then(|| label_parts.join(" ")),
            None,
        )
    };

    Ok(ResolvedDateRange {
        range,
        label,
        relative,
        effective_date: current_date,
    })
}

fn parse_home_arg(raw: &str) -> Result<PathBuf, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(rust_i18n::t!("cli.error.home_empty").into_owned());
    }
    let path = PathBuf::from(raw);
    if !path.is_dir() {
        return Err(rust_i18n::t!("cli.error.home_not_dir", path = path.display()).into_owned());
    }
    path.canonicalize().map_err(|error| {
        rust_i18n::t!(
            "cli.error.home_canonicalize",
            path = path.display(),
            error = error
        )
        .into_owned()
    })
}

fn parse_date_arg(raw: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .map_err(|_| rust_i18n::t!("cli.error.invalid_date", raw = raw).into_owned())
}

fn parse_year_arg(raw: &str) -> Result<i32, String> {
    if raw.len() != 4 || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(rust_i18n::t!("cli.error.invalid_year", raw = raw).into_owned());
    }
    let year = raw
        .parse::<i32>()
        .map_err(|_| rust_i18n::t!("cli.error.invalid_year", raw = raw).into_owned())?;
    NaiveDate::from_ymd_opt(year, 1, 1)
        .ok_or_else(|| rust_i18n::t!("cli.error.invalid_year", raw = raw).into_owned())?;
    Ok(year)
}

fn parse_positive_u64(raw: &str) -> Result<u64, String> {
    let value = raw
        .parse::<u64>()
        .map_err(|_| rust_i18n::t!("cli.error.invalid_refresh", raw = raw).into_owned())?;
    if value == 0 {
        return Err(rust_i18n::t!("cli.error.refresh_not_positive").into_owned());
    }
    Ok(value)
}
