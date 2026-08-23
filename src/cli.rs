/*!
Command-line interface for `rr`, modeled on ripgrep's `Flag`-trait mechanism.

Each optional flag is a zero-sized type implementing [`Flag`]; a single global
slice `FLAGS` holds them as `&dyn Flag`. The parser walks argv, looks a token
up in that slice, and calls [`Flag::update`] to fold the value into [`LowArgs`].
The same trait objects also generate `--help`, so documentation can't drift
from the parser. This is the faithful-but-minimal version of ripgrep's
`crates/core/flags`.
*/

use std::ffi::{OsStr, OsString};

use crate::marker::{self, Decoded};

/// The subcommand selected on the command line: the five verbs of
/// `[[rr:AD-3]]`. What each does for a user is `[[rr:help_text]]`, which
/// prints it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Subcommand {
    Index,
    Read,
    At,
    Search,
    Verify,
}

impl Subcommand {
    fn name(self) -> &'static str {
        match self {
            Subcommand::Index => "index",
            Subcommand::Read => "read",
            Subcommand::At => "at",
            Subcommand::Search => "search",
            Subcommand::Verify => "verify",
        }
    }

    fn from_token(tok: &OsStr) -> Option<Subcommand> {
        match tok.to_str()? {
            "index" => Some(Subcommand::Index),
            "read" => Some(Subcommand::Read),
            "at" => Some(Subcommand::At),
            "search" => Some(Subcommand::Search),
            "verify" => Some(Subcommand::Verify),
            _ => None,
        }
    }
}

/// Parse one reference token from the CLI into its bare anchor. A pasted
/// marker decodes (strip, then unescape, per `[[rr:AD-2]]`);
/// any other token already is a bare anchor. `Err` is a token that opens
/// like a marker but is not one: the user meant a marker, so it is a usage
/// error rather than a silent reparse.
pub(crate) fn parse_reference(token: &str) -> Result<String, String> {
    match marker::decode(token) {
        Decoded::Bare => Ok(token.to_string()),
        Decoded::Marker(anchor) => Ok(anchor),
        Decoded::Malformed(why) => Err(why),
    }
}

/// Split a qualified anchor `path#identity` at its first `#`
/// `[[rr:AD-1]]`. `None` when there is no `#` or either side is empty; the
/// caller tries the whole token as an identity first, so an identity that
/// itself contains `#` still resolves literally.
pub(crate) fn split_qualifier(anchor: &str) -> Option<(&str, &str)> {
    let (path, identity) = anchor.split_once('#')?;
    if path.is_empty() || identity.is_empty() {
        return None;
    }
    Some((path, identity))
}

/// A "special" mode that short-circuits normal dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Special {
    Help,
    Version,
}

/// Output format for the global `--format` flag `[[rr:AD-4]]`: text by
/// default, or one `rr-json` envelope per invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputFormat {
    Text,
    Json,
}

/// When to colorize, for the global `--color` flag. Parsed but inert: every
/// current output is plain text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Color {
    Auto,
    Always,
    Never,
}

/// The low-level, parsed-but-unresolved arguments. Mirrors ripgrep's
/// `LowArgs`: flags only validate into this struct; commands interpret it.
#[derive(Clone, Debug)]
pub(crate) struct LowArgs {
    pub command: Subcommand,
    pub index: Option<OsString>,
    pub format: OutputFormat,
    pub color: Color,
    /// `[[rr:QuietFlag]]`
    pub quiet: bool,
    /// `[[rr:NoFreshnessFlag]]`
    pub no_freshness: bool,
    /// `[[rr:AllFlag]]`
    pub all: bool,
    /// `[[rr:MentionsFlag]]`
    pub mentions: bool,
    /// `[[rr:MarkersFlag]]`
    pub markers: bool,
    /// Positional arguments (e.g. the anchor for `read`).
    pub positional: Vec<OsString>,
    /// Recorded rather than inferred from the fields above, so a flag's
    /// `verbs` is the only thing to keep right when one is added.
    pub seen_flags: Vec<&'static str>,
}

impl LowArgs {
    fn new(command: Subcommand) -> LowArgs {
        LowArgs {
            command,
            index: None,
            format: OutputFormat::Text,
            color: Color::Auto,
            quiet: false,
            no_freshness: false,
            all: false,
            mentions: false,
            markers: false,
            positional: Vec::new(),
            seen_flags: Vec::new(),
        }
    }
}

/// What [`parse`] resolves argv into.
pub(crate) enum ParseOutcome {
    Run(LowArgs),
    Special(Special),
}

#[derive(Debug)]
pub(crate) enum FlagValue {
    Switch,
    Value(OsString),
}

impl FlagValue {
    fn into_value(self, long: &str) -> Result<OsString, String> {
        match self {
            FlagValue::Value(v) => Ok(v),
            FlagValue::Switch => {
                Err(format!("flag --{long} requires a value"))
            }
        }
    }
}

pub(crate) trait Flag: Sync {
    /// True if the flag is a switch (takes no value).
    fn is_switch(&self) -> bool;
    /// Single-byte short name, if any (e.g. `q` for `-q`).
    fn name_short(&self) -> Option<char> {
        None
    }
    /// Long name (required), without the leading `--`.
    fn name_long(&self) -> &'static str;
    /// Empty means shared, the set `[[rr:Shared options]]` lists; on any
    /// other verb a scoped flag is the unknown flag of `[[rr:AD-4]]`. The
    /// same list writes the help prefix, so the two cannot disagree.
    fn verbs(&self) -> &'static [Subcommand] {
        &[]
    }
    /// Spelling that negates this flag, without the leading `--`. The
    /// negated name takes no value whatever the flag's own arity, and reaches
    /// `update` as `FlagValue::Switch`.
    fn name_negated(&self) -> Option<&'static str> {
        None
    }
    /// Accepted values, the default first; empty for a switch or a
    /// free-form value. The same list writes the help clause and the
    /// rejection message, so the two cannot disagree.
    fn doc_choices(&self) -> &'static [&'static str] {
        &[]
    }
    /// Terse one-line help string, without the verb prefix `verbs` writes.
    fn doc_short(&self) -> &'static str;
    /// Fold a parsed value into the low-level args.
    fn update(
        &self,
        value: FlagValue,
        args: &mut LowArgs,
    ) -> Result<(), String>;
}

struct IndexFlag;
impl Flag for IndexFlag {
    fn is_switch(&self) -> bool {
        false
    }
    fn name_long(&self) -> &'static str {
        "index"
    }
    fn doc_short(&self) -> &'static str {
        "Index path; else RIPREF_INDEX, else .ref-cache/index."
    }
    fn update(
        &self,
        value: FlagValue,
        args: &mut LowArgs,
    ) -> Result<(), String> {
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
    fn doc_choices(&self) -> &'static [&'static str] {
        &["text", "json"]
    }
    fn doc_short(&self) -> &'static str {
        "Output format."
    }
    fn update(
        &self,
        value: FlagValue,
        args: &mut LowArgs,
    ) -> Result<(), String> {
        let v = value.into_value(self.name_long())?;
        args.format = match v.to_str() {
            Some("text") => OutputFormat::Text,
            Some("json") => OutputFormat::Json,
            _ => {
                return Err(expected_one_of(
                    self.name_long(),
                    self.doc_choices(),
                    &v,
                ))
            }
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
    /// `[[rr:Shared options]]`
    fn name_negated(&self) -> Option<&'static str> {
        Some("no-color")
    }
    fn doc_choices(&self) -> &'static [&'static str] {
        &["auto", "always", "never"]
    }
    fn doc_short(&self) -> &'static str {
        "When to colorize."
    }
    fn update(
        &self,
        value: FlagValue,
        args: &mut LowArgs,
    ) -> Result<(), String> {
        let v = match value {
            FlagValue::Switch => {
                args.color = Color::Never;
                return Ok(());
            }
            FlagValue::Value(v) => v,
        };
        args.color = match v.to_str() {
            Some("auto") => Color::Auto,
            Some("always") => Color::Always,
            Some("never") => Color::Never,
            _ => {
                return Err(expected_one_of(
                    self.name_long(),
                    self.doc_choices(),
                    &v,
                ))
            }
        };
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
    fn doc_short(&self) -> &'static str {
        "Suppress the summary line; the answer still prints."
    }
    fn update(
        &self,
        _value: FlagValue,
        args: &mut LowArgs,
    ) -> Result<(), String> {
        args.quiet = true;
        Ok(())
    }
}

struct NoFreshnessFlag;
impl Flag for NoFreshnessFlag {
    fn is_switch(&self) -> bool {
        true
    }
    fn name_long(&self) -> &'static str {
        "no-freshness"
    }
    fn doc_short(&self) -> &'static str {
        "Skip the staleness check and answer from the index as-is."
    }
    fn update(
        &self,
        _value: FlagValue,
        args: &mut LowArgs,
    ) -> Result<(), String> {
        args.no_freshness = true;
        Ok(())
    }
}

struct AllFlag;
impl Flag for AllFlag {
    fn is_switch(&self) -> bool {
        true
    }
    fn name_long(&self) -> &'static str {
        "all"
    }
    fn verbs(&self) -> &'static [Subcommand] {
        &[Subcommand::At]
    }
    fn doc_short(&self) -> &'static str {
        "Report the whole covering nest, outermost first."
    }
    fn update(
        &self,
        _value: FlagValue,
        args: &mut LowArgs,
    ) -> Result<(), String> {
        args.all = true;
        Ok(())
    }
}

struct MentionsFlag;
impl Flag for MentionsFlag {
    fn is_switch(&self) -> bool {
        true
    }
    fn name_long(&self) -> &'static str {
        "mentions"
    }
    fn verbs(&self) -> &'static [Subcommand] {
        &[Subcommand::Search]
    }
    fn doc_short(&self) -> &'static str {
        "List path mentions instead of markers."
    }
    fn update(
        &self,
        _value: FlagValue,
        args: &mut LowArgs,
    ) -> Result<(), String> {
        args.mentions = true;
        Ok(())
    }
}

struct MarkersFlag;
impl Flag for MarkersFlag {
    fn is_switch(&self) -> bool {
        true
    }
    fn name_long(&self) -> &'static str {
        "markers"
    }
    fn verbs(&self) -> &'static [Subcommand] {
        &[Subcommand::Search]
    }
    fn doc_short(&self) -> &'static str {
        "List every marker, taking no <anchor>."
    }
    fn update(
        &self,
        _value: FlagValue,
        args: &mut LowArgs,
    ) -> Result<(), String> {
        args.markers = true;
        Ok(())
    }
}

static FLAGS: &[&dyn Flag] = &[
    &IndexFlag,
    &FormatFlag,
    &ColorFlag,
    &QuietFlag,
    &NoFreshnessFlag,
    &AllFlag,
    &MentionsFlag,
    &MarkersFlag,
];

fn lookup_long(name: &str) -> Option<&'static dyn Flag> {
    FLAGS
        .iter()
        .copied()
        .find(|f| f.name_long() == name || f.name_negated() == Some(name))
}

fn expected_one_of(
    long: &str,
    choices: &[&'static str],
    got: &OsStr,
) -> String {
    let quoted: Vec<String> =
        choices.iter().map(|c| format!("'{c}'")).collect();
    let list = match quoted.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} or {b}"),
        [rest @ .., last] => format!("{}, or {last}", rest.join(", ")),
    };
    format!("--{long} expects {list}, got '{}'", got.to_string_lossy())
}

fn lookup_short(ch: char) -> Option<&'static dyn Flag> {
    FLAGS.iter().copied().find(|f| f.name_short() == Some(ch))
}

/// `argv` is already stripped of the leading program name.
pub(crate) fn parse(argv: &[OsString]) -> Result<ParseOutcome, String> {
    let mut iter = argv.iter();
    let command = match iter.next() {
        None => {
            return Err("no command given (try 'index' or 'read')".to_string())
        }
        Some(tok) => match tok.to_str() {
            Some("-h") | Some("--help") => {
                return Ok(ParseOutcome::Special(Special::Help))
            }
            Some("-V") | Some("--version") => {
                return Ok(ParseOutcome::Special(Special::Version));
            }
            _ => Subcommand::from_token(tok).ok_or_else(|| {
                format!("unknown command: '{}'", tok.to_string_lossy())
            })?,
        },
    };
    let mut args = LowArgs::new(command);

    let mut positional_only = false;
    while let Some(tok) = iter.next() {
        // An inline value is cut from the raw bytes, never from a lossy
        // rendering: a path argv carries need not be UTF-8.
        let bytes = tok.as_encoded_bytes();
        let text = tok.to_string_lossy();
        if positional_only {
            args.positional.push(tok.clone());
        } else if text == "--" {
            positional_only = true;
        } else if text == "-h" || text == "--help" {
            return Ok(ParseOutcome::Special(Special::Help));
        } else if text == "-V" || text == "--version" {
            return Ok(ParseOutcome::Special(Special::Version));
        } else if let Some(rest) = bytes.strip_prefix(b"--") {
            let (name, inline) = match rest.iter().position(|&b| b == b'=') {
                Some(eq) => {
                    (&rest[..eq], Some(os_string_from_bytes(&rest[eq + 1..])))
                }
                None => (rest, None),
            };
            let name = String::from_utf8_lossy(name);
            let flag = lookup_long(&name)
                .ok_or_else(|| format!("unknown flag: --{name}"))?;
            // A scope error names the spelling the caller typed, not the one
            // it negates.
            let (name, negated) = match flag.name_negated() {
                Some(negated) if negated == name => (negated, true),
                _ => (flag.name_long(), false),
            };
            let value = take_value(flag, name, negated, inline, &mut iter)?;
            flag.update(value, &mut args)?;
            args.seen_flags.push(name);
        } else if text.starts_with('-') && text != "-" {
            let ch = text.chars().nth(1).unwrap();
            let flag = lookup_short(ch)
                .ok_or_else(|| format!("unknown flag: -{ch}"))?;
            // Every registered short is one ASCII byte, so a value starts
            // at byte 2.
            let inline =
                (bytes.len() > 2).then(|| os_string_from_bytes(&bytes[2..]));
            let value =
                take_value(flag, flag.name_long(), false, inline, &mut iter)?;
            flag.update(value, &mut args)?;
            args.seen_flags.push(flag.name_long());
        } else {
            args.positional.push(tok.clone());
        }
    }

    validate(&args)?;
    Ok(ParseOutcome::Run(args))
}

fn os_string_from_bytes(bytes: &[u8]) -> OsString {
    // SAFETY: `bytes` is a suffix of one argv token's encoded bytes, cut only
    // after an ASCII `=` or an ASCII short name. The encoding is a
    // self-synchronizing superset of UTF-8, which `OsStr` documents as safe to
    // split on an ASCII boundary, so the suffix is itself well-formed.
    #[allow(unsafe_code)]
    let s = unsafe { OsStr::from_encoded_bytes_unchecked(bytes) };
    s.to_os_string()
}

fn take_value(
    flag: &dyn Flag,
    name: &str,
    negated: bool,
    inline: Option<OsString>,
    iter: &mut std::slice::Iter<'_, OsString>,
) -> Result<FlagValue, String> {
    if negated || flag.is_switch() {
        if inline.is_some() {
            return Err(format!(
                "flag --{name} is a switch and takes no value"
            ));
        }
        Ok(FlagValue::Switch)
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

fn verb_list(verbs: &[Subcommand]) -> String {
    verbs
        .iter()
        .map(|v| format!("rr {}", v.name()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `[[rr:AD-3#Decision outcome]]`
fn validate(args: &LowArgs) -> Result<(), String> {
    for name in &args.seen_flags {
        let Some(flag) = lookup_long(name) else {
            continue;
        };
        let verbs = flag.verbs();
        if !verbs.is_empty() && !verbs.contains(&args.command) {
            return Err(format!(
                "--{name} applies to {} only",
                verb_list(verbs)
            ));
        }
    }
    match args.command {
        Subcommand::Read => match args.positional.len() {
            0 => Err("read requires an <anchor> argument".to_string()),
            1 => Ok(()),
            _ => Err("read takes exactly one <anchor>".to_string()),
        },
        Subcommand::At => match args.positional.len() {
            0 => Err("at requires a <file>:<line> argument".to_string()),
            // Resolved before any verb reads the index, so a malformed
            // location answers as a usage error and not as a stale index.
            // [[rr:AD-4#Decision outcome]]
            1 => parse_position(&args.positional[0].to_string_lossy())
                .map(|_| ()),
            _ => Err("at takes exactly one <file>:<line>".to_string()),
        },
        Subcommand::Index => {
            if args.positional.is_empty() {
                Ok(())
            } else {
                Err("index takes no positional arguments".to_string())
            }
        }
        Subcommand::Search if args.markers && args.mentions => {
            Err("search takes --markers or --mentions, not both".to_string())
        }
        Subcommand::Search => Ok(()),
        Subcommand::Verify => Ok(()),
    }
}

/// Split a `<file>:<line>` location into its parts, per the location grammar
/// of `[[rr:AD-1]]`: the span is the numeric suffix after the last colon, so
/// a path containing a colon (a Windows drive) keeps its prefix. `at` takes
/// a single line, never a range.
pub(crate) fn parse_position(s: &str) -> Result<(String, u64), String> {
    let (file, line) = s
        .rsplit_once(':')
        .ok_or_else(|| format!("expected <file>:<line>, got '{s}'"))?;
    if file.is_empty() {
        return Err(format!("expected <file>:<line>, got '{s}'"));
    }
    let line = line.parse::<u64>().map_err(|_| {
        format!("line must be a number in <file>:<line>, got '{s}'")
    })?;
    Ok((file.replace('\\', "/"), line))
}

/// The index file to read or write: `--index`, else `RIPREF_INDEX`, else
/// `.ref-cache/index`.
/// `[[rr:Shared options]]`
pub(crate) fn index_path(args: &LowArgs) -> OsString {
    if let Some(p) = &args.index {
        return p.clone();
    }
    if let Some(p) = std::env::var_os("RIPREF_INDEX") {
        return p;
    }
    OsString::from(".ref-cache/index")
}

/// Generate `--help` text from the flag registry (`[[rr:Documentation]]`).
pub(crate) fn help_text() -> String {
    let mut out = String::new();
    out.push_str("rr - reference code and prose by stable anchors.\n\n");
    out.push_str("USAGE:\n    rr <command> [options] [args]\n\n");
    out.push_str("COMMANDS:\n");
    out.push_str(
        "    index    Build / refresh the index from the working tree (the only writer)\n",
    );
    out.push_str("    read     Resolve a marker or bare anchor to its definition locations\n");
    out.push_str("    at       Print the marker covering a file:line (--all: the whole nest)\n");
    out.push_str("    search [<anchor>|--markers|--mentions] [<path>...]\n");
    out.push_str("             List the markers scoped text writes, or the path mentions\n");
    out.push_str("    verify [<path>...]\n");
    out.push_str(
        "             Judge references in scoped text; findings exit 1\n\n",
    );
    out.push_str("OPTIONS:\n");
    for flag in FLAGS {
        let short = match flag.name_short() {
            Some(c) => format!("-{c}, "),
            None => "    ".to_string(),
        };
        let scope = match flag.verbs() {
            [] => String::new(),
            verbs => format!("{}: ", verb_list(verbs)),
        };
        let doc = match flag.doc_choices() {
            [] => flag.doc_short().to_string(),
            [default, rest @ ..] => {
                let mut doc = format!(
                    "{}: {default} (default)",
                    flag.doc_short().trim_end_matches('.')
                );
                for choice in rest {
                    doc.push_str(&format!(", {choice}"));
                }
                doc.push('.');
                doc
            }
        };
        out.push_str(&format!(
            "    {short}--{:<12} {scope}{doc}\n",
            flag.name_long()
        ));
        if let Some(negated) = flag.name_negated() {
            out.push_str(&format!(
                "        --{:<12} {scope}Negate --{}.\n",
                negated,
                flag.name_long()
            ));
        }
    }
    out.push_str("    -h, --help         Show this help\n");
    out.push_str("    -V, --version      Print version\n");
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
        assert_eq!(parse_run(&["read", "a"]).command, Subcommand::Read);
        assert_eq!(parse_run(&["at", "a.rs:1"]).command, Subcommand::At);
        assert_eq!(parse_run(&["search"]).command, Subcommand::Search);
        assert_eq!(parse_run(&["search", "a"]).command, Subcommand::Search);
        assert_eq!(parse_run(&["verify"]).command, Subcommand::Verify);
    }

    #[test]
    fn parse_position_splits_file_and_line() {
        assert_eq!(
            parse_position("src/a.rs:42").unwrap(),
            ("src/a.rs".to_string(), 42)
        );
        assert_eq!(parse_position("a:b:7").unwrap(), ("a:b".to_string(), 7));
        // A backslash separator is CLI input; it normalizes to `/`.
        assert_eq!(
            parse_position(r"C:\src\a.rs:9").unwrap(),
            ("C:/src/a.rs".to_string(), 9)
        );
        assert!(parse_position("no-line").is_err());
        assert!(parse_position("a.rs:1-3").is_err()); // a range is not a position
        assert!(parse_position(":5").is_err()); // empty file
    }

    #[test]
    fn parse_reference_decodes_markers_and_passes_bare() {
        assert_eq!(parse_reference("AD-42").unwrap(), "AD-42");
        assert_eq!(parse_reference("[[rr:AD-42]]").unwrap(), "AD-42");
        assert_eq!(
            parse_reference(r"[[rr:arr\[0\]]]").unwrap(),
            "arr[0]",
            "unescape is normative"
        );
        assert!(
            parse_reference("[[rr:a]]@a1b2c3d").is_err(),
            "no suffix ever"
        );
        assert!(parse_reference("[[rr:a").is_err(), "unterminated is usage");
        // Tokens that merely contain grammar bytes stay bare.
        assert_eq!(
            parse_reference("support@example.com").unwrap(),
            "support@example.com"
        );
    }

    #[test]
    fn split_qualifier_takes_the_first_hash() {
        assert_eq!(
            split_qualifier("src/cli.rs#parse_reference"),
            Some(("src/cli.rs", "parse_reference"))
        );
        assert_eq!(
            split_qualifier("doc/x.md#A: b#c"),
            Some(("doc/x.md", "A: b#c")),
            "identity may itself contain #"
        );
        assert_eq!(split_qualifier("plain"), None);
        assert_eq!(split_qualifier("#lead"), None);
        assert_eq!(split_qualifier("trail#"), None);
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
    fn help_after_a_double_dash_is_an_anchor() {
        let args = parse_run(&["read", "--", "--help"]);
        assert_eq!(args.positional, vec![OsString::from("--help")]);
        let args = parse_run(&["search", "--", "-V"]);
        assert_eq!(args.positional, vec![OsString::from("-V")]);
    }

    /// Windows has no safe way to build an ill-formed `OsString`.
    #[cfg(unix)]
    #[test]
    fn an_inline_value_keeps_bytes_that_are_not_utf8() {
        use std::os::unix::ffi::OsStringExt;

        let bad = OsString::from_vec(vec![b'i', 0x80, b'x']);
        let mut inline = OsString::from("--index=");
        inline.push(&bad);
        let argv = vec![OsString::from("index"), inline];
        let Ok(ParseOutcome::Run(args)) = parse(&argv) else {
            panic!("expected Run");
        };
        assert_eq!(
            args.index.unwrap(),
            bad,
            "a lossy rendering would substitute U+FFFD"
        );
    }

    #[test]
    fn value_flags_take_inline_or_next_arg() {
        assert_eq!(
            parse_run(&["read", "a", "--index=foo"]).index.unwrap(),
            "foo"
        );
        assert_eq!(
            parse_run(&["read", "a", "--index", "bar"]).index.unwrap(),
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
        assert!(parse_err(&["at", "a.rs:1", "--all=yes"]).contains("switch"));
        assert!(parse_err(&["index", "--format"]).contains("requires a value"));
        assert_eq!(
            parse_err(&["index", "--format", "xml"]),
            "--format expects 'text' or 'json', got 'xml'",
            "a rejected value is echoed in single quotes, never Debug-quoted"
        );
        assert_eq!(
            parse_err(&["index", "--color", "purple"]),
            "--color expects 'auto', 'always', or 'never', got 'purple'",
            "three choices take the serial comma"
        );
    }

    #[test]
    fn a_negated_name_is_a_switch_over_its_flag() {
        assert_eq!(parse_run(&["index", "--no-color"]).color, Color::Never);
        assert!(parse_err(&["index", "--no-color=never"]).contains("switch"));
        assert!(
            parse_err(&["index", "--no-format"]).contains("unknown flag"),
            "a negated spelling exists only where a flag declares one"
        );
        assert_eq!(
            parse_run(&["index", "--no-color"]).seen_flags,
            vec!["no-color"],
            "the spelling as typed, so a scope error can name it"
        );
    }

    #[test]
    fn only_value_flags_declare_a_negated_name() {
        for flag in FLAGS {
            assert!(
                !(flag.is_switch() && flag.name_negated().is_some()),
                "--{} is a switch: `update` cannot tell its negation apart",
                flag.name_long()
            );
        }
    }

    #[test]
    fn short_switch_and_double_dash_positional() {
        assert!(parse_run(&["index", "-q"]).quiet);
        let args = parse_run(&["read", "--", "-weird-anchor"]);
        assert_eq!(args.positional, vec![OsString::from("-weird-anchor")]);
    }

    #[test]
    fn a_scoped_flag_elsewhere_is_a_usage_error() {
        assert_eq!(
            parse_err(&["verify", "--markers"]),
            "--markers applies to rr search only"
        );
        assert_eq!(
            parse_err(&["read", "a", "--mentions"]),
            "--mentions applies to rr search only"
        );
        assert_eq!(
            parse_err(&["search", "--all"]),
            "--all applies to rr at only"
        );
        assert!(parse_run(&["search", "--no-freshness"]).no_freshness);
        assert_eq!(
            parse_run(&["search", "--index", "x"]).index.unwrap(),
            "x",
            "shared per the README's Shared options"
        );
    }

    #[test]
    fn selection_flags_parse() {
        assert!(parse_run(&["at", "a.rs:1", "--all"]).all);
        assert!(parse_run(&["search", "--mentions"]).mentions);
        assert!(parse_run(&["search", "--markers"]).markers);
        assert!(parse_run(&["read", "a", "--no-freshness"]).no_freshness);
    }

    #[test]
    fn arity_and_unknowns_are_usage_errors() {
        assert!(parse_err(&[]).contains("no command"));
        assert!(parse_err(&["frobnicate"]).contains("unknown command"));
        for dropped in ["cite", "track", "uncite", "untrack", "enforce"] {
            assert!(
                parse_err(&[dropped, "a"]).contains("unknown command"),
                "{dropped} must not parse"
            );
        }
        assert!(parse_err(&["read"]).contains("requires an <anchor>"));
        assert!(parse_err(&["read", "a", "b"]).contains("exactly one"));
        assert!(parse_err(&["at"]).contains("requires a <file>:<line>"));
        assert!(parse_err(&["at", "a.rs:1", "b.rs:2"]).contains("exactly one"));
        assert!(parse_err(&["at", "a.rs"]).contains("<file>:<line>"));
        assert!(parse_err(&["at", "a.rs:xyz"]).contains("number"));
        assert!(parse_err(&["index", "stray"]).contains("no positional"));
        assert_eq!(
            parse_run(&["search", "a", "b"]).positional,
            vec![OsString::from("a"), OsString::from("b")]
        );
        assert!(parse_err(&["search", "--markers", "--mentions"])
            .contains("not both"));
        assert_eq!(
            parse_run(&["verify", "stray"]).positional,
            vec![OsString::from("stray")]
        );
        assert!(parse_err(&["index", "--bogus"]).contains("unknown flag"));
        assert!(parse_err(&["index", "-z"]).contains("unknown flag"));
        for gone in ["--cite", "--locate"] {
            assert!(
                parse_err(&["at", "a.rs:1", gone]).contains("unknown flag"),
                "{gone} must not parse"
            );
        }
    }

    #[test]
    fn index_path_prefers_flag_then_default() {
        let mut args = LowArgs::new(Subcommand::Index);
        assert_eq!(index_path(&args), OsString::from(".ref-cache/index"));
        args.index = Some(OsString::from("custom/idx"));
        assert_eq!(index_path(&args), OsString::from("custom/idx"));
    }

    #[test]
    fn help_text_lists_every_flag_and_verb() {
        let help = help_text();
        for flag in FLAGS {
            assert!(
                help.contains(flag.name_long()),
                "help missing --{}",
                flag.name_long()
            );
            for choice in flag.doc_choices() {
                assert!(
                    help.contains(choice),
                    "help missing {choice} for --{}",
                    flag.name_long()
                );
            }
            if let Some(negated) = flag.name_negated() {
                assert!(help.contains(negated), "help missing --{negated}");
            }
        }
        assert!(
            help.contains("auto (default), always, never"),
            "the choice list writes the clause, defaulting to the first"
        );
        for line in help.lines() {
            assert!(line.len() <= 80, "help line over 80 columns: {line}");
        }
        for verb in ["index", "read", "at", "search", "verify"] {
            assert!(help.contains(verb), "help missing verb {verb}");
        }
        for gone in ["cite", "track", "uncite", "untrack"] {
            assert!(
                !help.contains(&format!("\n    {gone} ")),
                "help must not advertise {gone}"
            );
        }
    }
}
