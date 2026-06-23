/*!
Command-line interface for `rr`, modeled on ripgrep's `Flag`-trait mechanism.

Each optional flag is a zero-sized type implementing [`Flag`]; a single global
slice `FLAGS` holds them as `&dyn Flag`. The parser walks argv, looks a token
up in that slice, and calls [`Flag::update`] to fold the value into [`LowArgs`].
The same trait objects also generate `--help`, so documentation can't drift from
the parser. This is the faithful-but-minimal version of ripgrep's
`crates/core/flags`; the doc-category / doc-short strings here are written to
become the eventual generated `--help` and man-page literals.
*/

use std::ffi::{OsStr, OsString};

/// The subcommand selected on the command line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subcommand {
    /// `rr index` — the sole writer.
    Index,
    /// `rr read` — dereference one anchor.
    Read,
    /// `rr at` — list the anchors whose span covers a `file:line` position.
    At,
}

impl Subcommand {
    fn from_token(tok: &OsStr) -> Option<Subcommand> {
        match tok.to_str()? {
            "index" => Some(Subcommand::Index),
            "read" => Some(Subcommand::Read),
            "at" => Some(Subcommand::At),
            _ => None,
        }
    }
}

/// A "special" mode that short-circuits normal dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Special {
    Help,
    Version,
}

/// Output format for the global `--format` flag. Only `Text` is emitted today;
/// `Json` parses but is not yet wired to output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

/// When to colorize, for the global `--color` flag. Parsed but currently inert:
/// `read --locate` output is a plain location line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    Auto,
    Always,
    Never,
}

/// The low-level, parsed-but-unresolved arguments. Mirrors ripgrep's `LowArgs`:
/// flags only validate into this struct; commands interpret it.
#[derive(Clone, Debug)]
pub struct LowArgs {
    pub command: Subcommand,
    pub index: Option<OsString>,
    pub format: OutputFormat,
    pub color: Color,
    /// `rr read --locate`: print only `file:start-end`.
    pub locate: bool,
    /// `rr index -q/--quiet`: suppress the summary line.
    pub quiet: bool,
    /// Positional arguments (e.g. the anchor for `read`).
    pub positional: Vec<OsString>,
}

impl LowArgs {
    fn new(command: Subcommand) -> LowArgs {
        LowArgs {
            command,
            index: None,
            format: OutputFormat::Text,
            color: Color::Auto,
            locate: false,
            quiet: false,
            positional: Vec::new(),
        }
    }
}

/// What [`parse`] resolves argv into.
pub enum ParseOutcome {
    Run(LowArgs),
    Special(Special),
}

/// A value parsed from the command line for a flag: a switch, or a value.
#[derive(Debug)]
pub enum FlagValue {
    Switch(bool),
    Value(OsString),
}

impl FlagValue {
    fn into_value(self, long: &str) -> Result<OsString, String> {
        match self {
            FlagValue::Value(v) => Ok(v),
            FlagValue::Switch(_) => Err(format!("flag --{long} requires a value")),
        }
    }
}

/// The definition of one optional flag. A trimmed-down [ripgrep `Flag`] trait:
/// long name (required), optional short name, whether it takes a value, the
/// documentation strings, and an `update` that folds the value into [`LowArgs`].
///
/// [ripgrep `Flag`]: https://github.com/BurntSushi/ripgrep
pub trait Flag: Sync {
    /// True if the flag is a switch (takes no value).
    fn is_switch(&self) -> bool;
    /// Single-byte short name, if any (e.g. `q` for `-q`).
    fn name_short(&self) -> Option<char> {
        None
    }
    /// Long name (required), without the leading `--`.
    fn name_long(&self) -> &'static str;
    /// Documentation category, pre-staging the eventual `doc_category`.
    fn doc_category(&self) -> &'static str;
    /// Terse one-line help string.
    fn doc_short(&self) -> &'static str;
    /// Fold a parsed value into the low-level args.
    fn update(&self, value: FlagValue, args: &mut LowArgs) -> Result<(), String>;
}

// --- Flag definitions ------------------------------------------------------

struct IndexFlag;
impl Flag for IndexFlag {
    fn is_switch(&self) -> bool {
        false
    }
    fn name_long(&self) -> &'static str {
        "index"
    }
    fn doc_category(&self) -> &'static str {
        "input"
    }
    fn doc_short(&self) -> &'static str {
        "Path to the index file (default .ref-cache/index)."
    }
    fn update(&self, value: FlagValue, args: &mut LowArgs) -> Result<(), String> {
        args.index = Some(value.into_value(self.name_long())?);
        Ok(())
    }
}

struct FormatFlag;
impl Flag for FormatFlag {
    fn is_switch(&self) -> bool {
        false
    }
    fn name_long(&self) -> &'static str {
        "format"
    }
    fn doc_category(&self) -> &'static str {
        "output"
    }
    fn doc_short(&self) -> &'static str {
        "Output format: text (default) or json."
    }
    fn update(&self, value: FlagValue, args: &mut LowArgs) -> Result<(), String> {
        let v = value.into_value(self.name_long())?;
        args.format = match v.to_str() {
            Some("text") => OutputFormat::Text,
            Some("json") => OutputFormat::Json,
            _ => return Err(format!("--format expects 'text' or 'json', got {v:?}")),
        };
        Ok(())
    }
}

struct ColorFlag;
impl Flag for ColorFlag {
    fn is_switch(&self) -> bool {
        false
    }
    fn name_long(&self) -> &'static str {
        "color"
    }
    fn doc_category(&self) -> &'static str {
        "output"
    }
    fn doc_short(&self) -> &'static str {
        "When to colorize: auto (default), always, never."
    }
    fn update(&self, value: FlagValue, args: &mut LowArgs) -> Result<(), String> {
        let v = value.into_value(self.name_long())?;
        args.color = match v.to_str() {
            Some("auto") => Color::Auto,
            Some("always") => Color::Always,
            Some("never") => Color::Never,
            _ => return Err(format!("--color expects auto|always|never, got {v:?}")),
        };
        Ok(())
    }
}

struct NoColorFlag;
impl Flag for NoColorFlag {
    fn is_switch(&self) -> bool {
        true
    }
    fn name_long(&self) -> &'static str {
        "no-color"
    }
    fn doc_category(&self) -> &'static str {
        "output"
    }
    fn doc_short(&self) -> &'static str {
        "Disable colored output (= --color never)."
    }
    fn update(&self, _value: FlagValue, args: &mut LowArgs) -> Result<(), String> {
        args.color = Color::Never;
        Ok(())
    }
}

struct LocateFlag;
impl Flag for LocateFlag {
    fn is_switch(&self) -> bool {
        true
    }
    fn name_long(&self) -> &'static str {
        "locate"
    }
    fn doc_category(&self) -> &'static str {
        "output"
    }
    fn doc_short(&self) -> &'static str {
        "Print only the resolved location (file:start-end)."
    }
    fn update(&self, _value: FlagValue, args: &mut LowArgs) -> Result<(), String> {
        args.locate = true;
        Ok(())
    }
}

struct QuietFlag;
impl Flag for QuietFlag {
    fn is_switch(&self) -> bool {
        true
    }
    fn name_short(&self) -> Option<char> {
        Some('q')
    }
    fn name_long(&self) -> &'static str {
        "quiet"
    }
    fn doc_category(&self) -> &'static str {
        "logging"
    }
    fn doc_short(&self) -> &'static str {
        "Suppress the summary line on success."
    }
    fn update(&self, _value: FlagValue, args: &mut LowArgs) -> Result<(), String> {
        args.quiet = true;
        Ok(())
    }
}

/// The global flag registry: every optional flag, as a trait object.
static FLAGS: &[&dyn Flag] = &[
    &IndexFlag,
    &FormatFlag,
    &ColorFlag,
    &NoColorFlag,
    &LocateFlag,
    &QuietFlag,
];

fn lookup_long(name: &str) -> Option<&'static dyn Flag> {
    FLAGS.iter().copied().find(|f| f.name_long() == name)
}

fn lookup_short(ch: char) -> Option<&'static dyn Flag> {
    FLAGS.iter().copied().find(|f| f.name_short() == Some(ch))
}

/// Parse argv (already stripped of the leading program name) into a [`ParseOutcome`].
pub fn parse(argv: &[OsString]) -> Result<ParseOutcome, String> {
    // `--help`/`-h` and `--version`/`-V` are special: honored anywhere, alone.
    for tok in argv {
        match tok.to_str() {
            Some("-h") | Some("--help") => return Ok(ParseOutcome::Special(Special::Help)),
            Some("-V") | Some("--version") => return Ok(ParseOutcome::Special(Special::Version)),
            _ => {}
        }
    }

    let mut iter = argv.iter();
    let command = match iter.next() {
        None => return Err("no command given (try 'index' or 'read')".to_string()),
        Some(tok) => Subcommand::from_token(tok)
            .ok_or_else(|| format!("unknown command: {}", tok.to_string_lossy()))?,
    };
    let mut args = LowArgs::new(command);

    let mut positional_only = false;
    while let Some(tok) = iter.next() {
        let text = tok.to_string_lossy();
        if positional_only {
            args.positional.push(tok.clone());
        } else if text == "--" {
            positional_only = true;
        } else if let Some(rest) = text.strip_prefix("--") {
            // Long flag, possibly `--name=value`.
            let (name, inline) = match rest.split_once('=') {
                Some((n, v)) => (n, Some(OsString::from(v))),
                None => (rest, None),
            };
            let flag = lookup_long(name).ok_or_else(|| format!("unknown flag: --{name}"))?;
            let value = take_value(flag, name, inline, &mut iter)?;
            flag.update(value, &mut args)?;
        } else if text.starts_with('-') && text != "-" {
            // Short flag(s). Only single switches / `-x value` are supported.
            let ch = text.chars().nth(1).unwrap();
            let flag = lookup_short(ch).ok_or_else(|| format!("unknown flag: -{ch}"))?;
            let inline = if text.len() > 2 {
                Some(OsString::from(&text[2..]))
            } else {
                None
            };
            let value = take_value(flag, flag.name_long(), inline, &mut iter)?;
            flag.update(value, &mut args)?;
        } else {
            args.positional.push(tok.clone());
        }
    }

    validate(&args)?;
    Ok(ParseOutcome::Run(args))
}

fn take_value(
    flag: &dyn Flag,
    name: &str,
    inline: Option<OsString>,
    iter: &mut std::slice::Iter<'_, OsString>,
) -> Result<FlagValue, String> {
    if flag.is_switch() {
        if inline.is_some() {
            return Err(format!("flag --{name} is a switch and takes no value"));
        }
        Ok(FlagValue::Switch(true))
    } else {
        let v = match inline {
            Some(v) => v,
            None => iter
                .next()
                .cloned()
                .ok_or_else(|| format!("flag --{name} requires a value"))?,
        };
        Ok(FlagValue::Value(v))
    }
}

fn validate(args: &LowArgs) -> Result<(), String> {
    match args.command {
        Subcommand::Read => match args.positional.len() {
            0 => Err("read requires an <anchor> argument".to_string()),
            1 => Ok(()),
            _ => Err("read takes exactly one <anchor>".to_string()),
        },
        Subcommand::At => match args.positional.len() {
            0 => Err("at requires a <file>:<line> argument".to_string()),
            // Resolve the position now so a malformed `file:line` is a usage
            // error (exit 2) caught before any command touches the index.
            1 => parse_position(&args.positional[0].to_string_lossy()).map(|_| ()),
            _ => Err("at takes exactly one <file>:<line>".to_string()),
        },
        Subcommand::Index => {
            if args.positional.is_empty() {
                Ok(())
            } else {
                Err("index takes no positional arguments".to_string())
            }
        }
    }
}

/// Split a `<file>:<line>` position into its parts. Parses the line from the
/// right (`rsplit_once`) so a path containing a colon keeps its prefix; the line
/// must be a bare `u64`. The richer `file:start-end` (range) and `file:line:col`
/// (column) forms are reserved as provisional and are not accepted yet — a
/// range tail fails the numeric parse and reports a usage error.
pub fn parse_position(s: &str) -> Result<(String, u64), String> {
    let (file, line) = s
        .rsplit_once(':')
        .ok_or_else(|| format!("expected <file>:<line>, got {s:?}"))?;
    if file.is_empty() {
        return Err(format!("expected <file>:<line>, got {s:?}"));
    }
    let line = line
        .parse::<u64>()
        .map_err(|_| format!("line must be a number in <file>:<line>, got {s:?}"))?;
    Ok((file.to_string(), line))
}

/// Resolve the index path: `--index`, else `REF_INDEX`, else the default.
pub fn index_path(args: &LowArgs) -> OsString {
    if let Some(p) = &args.index {
        return p.clone();
    }
    if let Some(p) = std::env::var_os("REF_INDEX") {
        return p;
    }
    OsString::from(".ref-cache/index")
}

/// Generate `--help` text from the flag registry, proving the docs-from-flags
/// design. A later version replaces this with the fully generated help.
pub fn help_text() -> String {
    let mut out = String::new();
    out.push_str("rr — cite code and prose by stable anchors.\n\n");
    out.push_str("USAGE:\n    rr <command> [options] [args]\n\n");
    out.push_str("COMMANDS:\n");
    out.push_str("    index    Build / refresh the index from the working tree (writer)\n");
    out.push_str("    read     Dereference an anchor to the chunk it points at\n");
    out.push_str("    at       List the anchors whose span covers a file:line position\n\n");
    out.push_str("OPTIONS:\n");
    for flag in FLAGS {
        let short = match flag.name_short() {
            Some(c) => format!("-{c}, "),
            None => "    ".to_string(),
        };
        out.push_str(&format!(
            "    {short}--{:<10} {}\n",
            flag.name_long(),
            flag.doc_short()
        ));
    }
    out.push_str("    -h, --help       Show this help\n");
    out.push_str("    -V, --version    Print version\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_run(args: &[&str]) -> LowArgs {
        let argv: Vec<OsString> = args.iter().map(OsString::from).collect();
        match parse(&argv) {
            Ok(ParseOutcome::Run(a)) => a,
            other => panic!("expected Run, got {:?}", other.err()),
        }
    }

    fn parse_err(args: &[&str]) -> String {
        let argv: Vec<OsString> = args.iter().map(OsString::from).collect();
        match parse(&argv) {
            Err(e) => e,
            Ok(_) => panic!("expected an error for {args:?}"),
        }
    }

    #[test]
    fn picks_the_command() {
        assert_eq!(parse_run(&["index"]).command, Subcommand::Index);
        assert_eq!(parse_run(&["read", "a.rs"]).command, Subcommand::Read);
        assert_eq!(parse_run(&["at", "a.rs:1"]).command, Subcommand::At);
    }

    #[test]
    fn parse_position_splits_file_and_line() {
        assert_eq!(
            parse_position("src/a.rs:42").unwrap(),
            ("src/a.rs".to_string(), 42)
        );
        // Right-split keeps a colon-bearing prefix attached to the file.
        assert_eq!(parse_position("a:b:7").unwrap(), ("a:b".to_string(), 7));
        assert!(parse_position("no-line").is_err());
        assert!(parse_position("a.rs:1-3").is_err()); // range form not accepted yet
        assert!(parse_position(":5").is_err()); // empty file
    }

    #[test]
    fn help_and_version_short_circuit_anywhere() {
        for a in [&["--help"][..], &["read", "--help"], &["-h"]] {
            assert!(matches!(
                parse(&a.iter().map(OsString::from).collect::<Vec<_>>()),
                Ok(ParseOutcome::Special(Special::Help))
            ));
        }
        assert!(matches!(
            parse(&[OsString::from("-V")]),
            Ok(ParseOutcome::Special(Special::Version))
        ));
    }

    #[test]
    fn value_flags_take_inline_or_next_arg() {
        assert_eq!(
            parse_run(&["read", "a.rs", "--index=foo"]).index.unwrap(),
            "foo"
        );
        assert_eq!(
            parse_run(&["read", "a.rs", "--index", "bar"])
                .index
                .unwrap(),
            "bar"
        );
    }

    #[test]
    fn format_and_color_parse_their_choices() {
        assert_eq!(
            parse_run(&["index", "--format", "json"]).format,
            OutputFormat::Json
        );
        assert_eq!(parse_run(&["index", "--color=never"]).color, Color::Never);
        assert_eq!(parse_run(&["index", "--no-color"]).color, Color::Never);
    }

    #[test]
    fn switches_reject_values_and_value_flags_require_them() {
        assert!(parse_err(&["read", "a.rs", "--locate=yes"]).contains("switch"));
        assert!(parse_err(&["index", "--format"]).contains("requires a value"));
        assert!(parse_err(&["index", "--format", "xml"]).contains("text"));
    }

    #[test]
    fn short_switch_and_double_dash_positional() {
        assert!(parse_run(&["index", "-q"]).quiet);
        // `--` forces the rest to be positional, even a leading-dash anchor.
        let args = parse_run(&["read", "--", "-weird-anchor"]);
        assert_eq!(args.positional, vec![OsString::from("-weird-anchor")]);
    }

    #[test]
    fn arity_and_unknowns_are_usage_errors() {
        assert!(parse_err(&[]).contains("no command"));
        assert!(parse_err(&["frobnicate"]).contains("unknown command"));
        assert!(parse_err(&["read"]).contains("requires an <anchor>"));
        assert!(parse_err(&["read", "a", "b"]).contains("exactly one"));
        assert!(parse_err(&["at"]).contains("requires a <file>:<line>"));
        assert!(parse_err(&["at", "a.rs:1", "b.rs:2"]).contains("exactly one"));
        assert!(parse_err(&["at", "a.rs"]).contains("<file>:<line>"));
        assert!(parse_err(&["at", "a.rs:xyz"]).contains("number"));
        assert!(parse_err(&["index", "stray"]).contains("no positional"));
        assert!(parse_err(&["index", "--bogus"]).contains("unknown flag"));
        assert!(parse_err(&["index", "-z"]).contains("unknown flag"));
    }

    #[test]
    fn index_path_prefers_flag_then_default() {
        let mut args = LowArgs::new(Subcommand::Index);
        assert_eq!(index_path(&args), OsString::from(".ref-cache/index"));
        args.index = Some(OsString::from("custom/idx"));
        assert_eq!(index_path(&args), OsString::from("custom/idx"));
    }

    #[test]
    fn help_text_lists_every_flag() {
        let help = help_text();
        for flag in FLAGS {
            assert!(
                help.contains(flag.name_long()),
                "help missing --{}",
                flag.name_long()
            );
        }
    }
}
