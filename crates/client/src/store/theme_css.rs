//! What a personal theme's custom CSS can do in this app: set the documented
//! appearance variables on the root (`themeVariables` in the web client's
//! `shared/theme-css.ts`). There is no page here, so rules that style
//! elements by selector do nothing; they are counted, not applied.
//!
//! Read: declarations of a rule whose selector list has `:root` or `html`
//! alone (as the web client's `rootSelector`), at the top level only.
//! A later declaration of the same variable wins, as in CSS.

use cssparser::{
    AtRuleParser, CowRcStr, DeclarationParser, ParseError, Parser, ParserState,
    QualifiedRuleParser, RuleBodyItemParser, RuleBodyParser, StyleSheetParser,
};

/// The colour tokens a theme may set, by their web names (`themeTokens`).
pub const COLOR_VARIABLES: [&str; 28] = [
    "rail",
    "sidebar",
    "main",
    "panel",
    "raised",
    "hover",
    "input",
    "border",
    "control-border",
    "text",
    "body",
    "muted",
    "faint",
    "accent",
    "accent-ink",
    "selection",
    "mint",
    "danger",
    "yellow",
    "code-keyword",
    "code-string",
    "code-number",
    "code-comment",
    "code-function",
    "code-type",
    "code-property",
    "code-meta",
    "code-deletion",
];

/// The other documented variables.
pub const OTHER_VARIABLES: [&str; 8] = [
    "font-ui",
    "font-message",
    "font-mono",
    "message-size",
    "group-gap",
    "radius",
    "radius-large",
    "interface-scale",
];

/// What a theme's CSS sets, ready to lay over the compiled appearance.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CssVariables {
    /// Colour tokens by web name, `0xRRGGBB`.
    pub colors: Vec<(&'static str, u32)>,
    /// Font families in the order written, generic names included.
    pub font_ui: Vec<String>,
    pub font_message: Vec<String>,
    pub font_mono: Vec<String>,
    /// Pixels.
    pub message_size: Option<f32>,
    pub group_gap: Option<f32>,
    pub radius: Option<f32>,
    pub radius_large: Option<f32>,
    /// 1.0 = 100 %.
    pub interface_scale: Option<f32>,
    /// Documented variables whose value this app could not read
    /// (`color-mix()`, a translucent colour, a `calc()`), without the `--`.
    pub unread: Vec<String>,
    /// Rules that style elements or are at-rules: not applied here.
    pub skipped_rules: usize,
}

impl CssVariables {
    /// Names of the variables applied, without the `--`, in a fixed order.
    pub fn applied(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = self.colors.iter().map(|(n, _)| *n).collect();
        for (name, set) in [
            ("font-ui", !self.font_ui.is_empty()),
            ("font-message", !self.font_message.is_empty()),
            ("font-mono", !self.font_mono.is_empty()),
            ("message-size", self.message_size.is_some()),
            ("group-gap", self.group_gap.is_some()),
            ("radius", self.radius.is_some()),
            ("radius-large", self.radius_large.is_some()),
            ("interface-scale", self.interface_scale.is_some()),
        ] {
            if set {
                out.push(name);
            }
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.applied().is_empty()
    }
}

/// Every custom property a root rule set, in order, as written.
struct Sheet {
    declared: Vec<(String, String)>,
    skipped: usize,
}

struct Declarations<'a> {
    out: &'a mut Vec<(String, String)>,
}

impl<'i> DeclarationParser<'i> for Declarations<'_> {
    type Declaration = ();
    type Error = ();

    fn parse_value(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i>,
        _start: &ParserState,
    ) -> Result<(), ParseError<()>> {
        let start = input.position();
        while input.next().is_ok() {}
        let value = input.slice_from(start).trim();
        if let Some(name) = name.strip_prefix("--") {
            let value = value.trim_end_matches("!important").trim();
            self.out.push((name.to_string(), value.to_string()));
        }
        Ok(())
    }
}

impl<'i> AtRuleParser<'i> for Declarations<'_> {
    type Prelude = ();
    type AtRule = ();
    type Error = ();
}

impl<'i> QualifiedRuleParser<'i> for Declarations<'_> {
    type Prelude = ();
    type QualifiedRule = ();
    type Error = ();
}

impl<'i> RuleBodyItemParser<'i, (), ()> for Declarations<'_> {
    fn parse_declarations(&self) -> bool {
        true
    }
    fn parse_qualified(&self) -> bool {
        false
    }
}

struct Rules<'a> {
    sheet: &'a mut Sheet,
}

impl<'i> QualifiedRuleParser<'i> for Rules<'_> {
    type Prelude = bool;
    type QualifiedRule = ();
    type Error = ();

    fn parse_prelude(&mut self, input: &mut Parser<'i>) -> Result<bool, ParseError<()>> {
        let start = input.position();
        while input.next().is_ok() {}
        Ok(targets_root(input.slice_from(start)))
    }

    fn parse_block(
        &mut self,
        root: bool,
        _start: &ParserState,
        input: &mut Parser<'i>,
    ) -> Result<(), ParseError<()>> {
        if !root {
            self.sheet.skipped += 1;
            while input.next().is_ok() {}
            return Ok(());
        }
        let mut declarations = Declarations {
            out: &mut self.sheet.declared,
        };
        for item in RuleBodyParser::new(input, &mut declarations) {
            let _ = item;
        }
        Ok(())
    }
}

impl<'i> AtRuleParser<'i> for Rules<'_> {
    type Prelude = ();
    type AtRule = ();
    type Error = ();

    fn parse_prelude(
        &mut self,
        _name: CowRcStr<'i>,
        input: &mut Parser<'i>,
    ) -> Result<(), ParseError<()>> {
        while input.next().is_ok() {}
        Ok(())
    }

    fn rule_without_block(&mut self, _prelude: (), _start: &ParserState) -> Result<(), ()> {
        self.sheet.skipped += 1;
        Ok(())
    }

    fn parse_block(
        &mut self,
        _prelude: (),
        _start: &ParserState,
        input: &mut Parser<'i>,
    ) -> Result<(), ParseError<()>> {
        self.sheet.skipped += 1;
        while input.next().is_ok() {}
        Ok(())
    }
}

/// `:root`, `html` or `html:root` as one of the selectors (`rootSelector`).
fn targets_root(selectors: &str) -> bool {
    selectors.split(',').any(|part| {
        let part = part.trim().to_ascii_lowercase();
        let rest = part
            .strip_prefix(":root")
            .or_else(|| part.strip_prefix("html"));
        rest.is_some_and(|mut rest| {
            while let Some(more) = rest.strip_prefix(":root") {
                rest = more;
            }
            rest.is_empty()
        })
    })
}

/// The documented variables `css` sets on the root. `resolve` gives the
/// current colour of a token the CSS refers to but does not set itself
/// (`--accent: var(--mint)`).
pub fn theme_variables(css: &str, resolve: &dyn Fn(&str) -> Option<u32>) -> CssVariables {
    let mut sheet = Sheet {
        declared: Vec::new(),
        skipped: 0,
    };
    let mut parser = Parser::new(css);
    let mut rules = Rules { sheet: &mut sheet };
    for rule in StyleSheetParser::new(&mut parser, &mut rules) {
        if rule.is_err() {
            // The parser already skipped what it could not read.
        }
    }
    let declared = &sheet.declared;
    let lookup = |name: &str| find(declared, name);
    let mut out = CssVariables {
        skipped_rules: sheet.skipped,
        ..Default::default()
    };
    for name in COLOR_VARIABLES {
        let Some(value) = lookup(name) else { continue };
        match color(value, declared, resolve, 0) {
            Some(rgb) => out.colors.push((name, rgb)),
            None => out.unread.push(name.to_string()),
        }
    }
    for (name, slot) in [
        ("font-ui", &mut out.font_ui),
        ("font-message", &mut out.font_message),
        ("font-mono", &mut out.font_mono),
    ] {
        if let Some(value) = lookup(name) {
            *slot = families(value);
            if slot.is_empty() {
                out.unread.push(name.to_string());
            }
        }
    }
    // The same ranges the settings allow (`store/appearance.rs`).
    for (name, slot, range) in [
        ("message-size", &mut out.message_size, (12.0, 22.0)),
        ("group-gap", &mut out.group_gap, (0.0, 32.0)),
        ("radius", &mut out.radius, (0.0, 24.0)),
        ("radius-large", &mut out.radius_large, (0.0, 32.0)),
    ] {
        if let Some(value) = lookup(name) {
            match length(value) {
                Some(px) => *slot = Some(px.clamp(range.0, range.1)),
                None => out.unread.push(name.to_string()),
            }
        }
    }
    if let Some(value) = lookup("interface-scale") {
        match value.trim().parse::<f32>() {
            Ok(scale) if scale.is_finite() => out.interface_scale = Some(scale.clamp(0.8, 1.5)),
            _ => out.unread.push("interface-scale".to_string()),
        }
    }
    out
}

/// An opaque colour: any CSS colour `csscolorparser` reads, or `var(--x)`
/// naming another variable this CSS sets or a token of the app.
/// The last value `declared` gives `name`.
fn find<'a>(declared: &'a [(String, String)], name: &str) -> Option<&'a str> {
    declared
        .iter()
        .rev()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

fn color(
    value: &str,
    declared: &[(String, String)],
    resolve: &dyn Fn(&str) -> Option<u32>,
    depth: usize,
) -> Option<u32> {
    let value = value.trim();
    if let Some(inner) = value.strip_prefix("var(").and_then(|v| v.strip_suffix(')')) {
        if depth >= 8 {
            return None;
        }
        let (name, fallback) = match inner.split_once(',') {
            Some((name, fallback)) => (name.trim(), Some(fallback)),
            None => (inner.trim(), None),
        };
        let name = name.strip_prefix("--")?;
        return find(declared, name)
            .and_then(|v| color(v, declared, resolve, depth + 1))
            .or_else(|| resolve(name))
            .or_else(|| fallback.and_then(|f| color(f, declared, resolve, depth + 1)));
    }
    let parsed = csscolorparser::parse(value).ok()?;
    let [r, g, b, a] = parsed.to_rgba8();
    (a == 255).then_some(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b))
}

/// `"Inter", system-ui, sans-serif` → `["Inter", "system-ui", "sans-serif"]`.
fn families(value: &str) -> Vec<String> {
    if value.contains('(') {
        return Vec::new();
    }
    value
        .split(',')
        .map(|f| f.trim().trim_matches(['"', '\'']).trim().to_string())
        .filter(|f| !f.is_empty())
        .collect()
}

/// `8px`, `0.5rem` (16 px to the rem) or `0`.
fn length(value: &str) -> Option<f32> {
    let value = value.trim();
    let (number, scale) = if let Some(n) = value.strip_suffix("px") {
        (n, 1.0)
    } else if let Some(n) = value.strip_suffix("rem") {
        (n, 16.0)
    } else {
        (value, 1.0)
    };
    let parsed: f32 = number.trim().parse().ok()?;
    if value.ends_with("px") || value.ends_with("rem") || parsed == 0.0 {
        (parsed.is_finite() && parsed >= 0.0).then_some(parsed * scale)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(css: &str) -> CssVariables {
        theme_variables(css, &|name| (name == "mint").then_some(0x33cc99))
    }

    #[test]
    fn root_variables_are_read_and_other_rules_counted() {
        let v = read(
            r#"
            /* a theme */
            @import url("https://example.com/x.css");
            :root {
              --accent: #FF8800;
              --rail: rgb(10 20 30);
              --text: hsl(0, 0%, 100%);
              --danger: tomato;
              --radius: 12px;
              --radius-large: 1rem;
              --font-ui: "Inter", system-ui, sans-serif;
              --message-size: 17px;
              --interface-scale: 1.1;
              --not-documented: 4px;
            }
            .message:hover { background: red }
            @media (min-width: 600px) { :root { --accent: blue } }
            html { --sidebar: #123; --accent: #00ff00 !important }
            "#,
        );
        assert_eq!(
            v.colors,
            [
                ("rail", 0x0a141e),
                ("sidebar", 0x112233),
                ("text", 0xffffff),
                ("accent", 0x00ff00),
                ("danger", 0xff6347),
            ]
        );
        assert_eq!(v.radius, Some(12.0));
        assert_eq!(v.radius_large, Some(16.0));
        assert_eq!(v.font_ui, ["Inter", "system-ui", "sans-serif"]);
        assert_eq!(v.message_size, Some(17.0));
        assert_eq!(v.interface_scale, Some(1.1));
        assert_eq!(v.skipped_rules, 3, "@import, .message:hover, @media");
        assert!(v.unread.is_empty());
        assert_eq!(v.applied().len(), 10);
    }

    #[test]
    fn var_follows_the_css_then_the_app_and_unreadable_values_are_named() {
        let v = read(
            ":root { --brand: #010203; --accent: var(--brand); --main: var(--mint);
                     --panel: var(--missing, #040506); --hover: color-mix(in srgb, red, blue);
                     --border: rgba(0,0,0,.5); --radius: calc(4px * 2); --group-gap: 3em;
                     --font-mono: var(--x) }",
        );
        assert_eq!(
            v.colors,
            [
                ("main", 0x33cc99),
                ("panel", 0x040506),
                ("accent", 0x010203)
            ]
        );
        assert_eq!(
            v.unread,
            ["hover", "border", "font-mono", "group-gap", "radius"]
        );
    }

    #[test]
    fn selectors_other_than_the_bare_root_do_not_count() {
        for css in [
            "html.dark { --accent: red }",
            ":root .x { --accent: red }",
            "body { --accent: red }",
            ":root:hover { --accent: red }",
        ] {
            let v = read(css);
            assert!(v.is_empty(), "{css}");
            assert_eq!(v.skipped_rules, 1, "{css}");
        }
        assert_eq!(
            read("html:root, body { --accent: #f00 }").colors,
            [("accent", 0xff0000)]
        );
    }

    #[test]
    fn broken_css_does_not_panic_and_keeps_what_parsed() {
        let v = read(":root { --accent: #abc; --rail: } } garbage {{{ :root { --main: #000 }");
        assert_eq!(v.colors.first(), Some(&("accent", 0xaabbcc)));
        let _ = read("}}}}{{{{ \"unterminated");
        let _ = read(&"a{".repeat(5000));
    }
}
