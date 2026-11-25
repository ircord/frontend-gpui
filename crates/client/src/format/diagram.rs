//! A `mermaid` fence drawn as a diagram: the source to a safe SVG through
//! `merman` (a pure-Rust Mermaid parser and layout), with no browser, no
//! script and no network. The app turns the SVG into a picture; this module
//! has no GPUI and is tested without a window.
//!
//! Only the families in [`SUPPORTED`] are compiled in. Every other source,
//! a parse error, a source over [`MAX_SOURCE`] bytes and a layout that runs
//! past [`DEADLINE`] return a [`DiagramError`], and the app shows the source
//! as an ordinary code block instead.

use std::sync::Arc;
use std::time::Duration;

use merman::svg::{
    DeterministicTextMeasurer, MeasurementProfileId, RenderResourcePolicy, SvgPipeline,
    TextMeasurementPolicy, TextMeasurementProfile, TextMeasurementProfileIdentity,
};
use merman::{
    CancelReason, MermaidEngine, OperationControl, ParseOptions, RenderError, RenderOutput,
    RenderRequest, Renderer, SourcePresentationConfig, SvgEnvironment, SvgRequest,
};

/// Sources longer than this many bytes are not drawn.
pub const MAX_SOURCE: usize = 8192;

/// How long one diagram may take to lay out. An unoptimised build (tests,
/// development) lays out several times slower, so it waits longer.
pub const DEADLINE: Duration = if cfg!(debug_assertions) {
    Duration::from_secs(10)
} else {
    Duration::from_secs(2)
};

/// The diagram types drawn, by the keyword a source starts with. The other
/// Mermaid types (C4, sankey, block, architecture…) are not yet readable in
/// a host theme and show their source.
pub const SUPPORTED: &[&str] = &[
    "flowchart",
    "flowchart-elk",
    "graph",
    "sequenceDiagram",
    "classDiagram",
    "classDiagram-v2",
    "stateDiagram",
    "stateDiagram-v2",
    "erDiagram",
    "gantt",
    "pie",
    "gitGraph",
    "mindmap",
    "timeline",
    "quadrantChart",
    "xychart",
    "xychart-beta",
    "journey",
];

/// Whether a fence's language is a diagram.
pub fn is_diagram(lang: Option<&str>) -> bool {
    lang.is_some_and(|l| l.eq_ignore_ascii_case("mermaid"))
}

/// The colours and font a diagram is drawn in, as `#rrggbb`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DiagramTheme {
    pub dark: bool,
    /// The family the text is drawn and measured in.
    pub font: String,
    pub text: String,
    /// Boxes and their borders.
    pub node: String,
    pub border: String,
    /// Lines and arrows.
    pub line: String,
    /// The second and third fills (clusters, notes, alternate rows).
    pub secondary: String,
    pub tertiary: String,
    /// Behind the diagram; the picture itself stays transparent.
    pub background: String,
}

/// The width of `text` in pixels at `size`, bold or not, in the drawn font.
pub type Measure = Arc<dyn Fn(&str, f64, bool) -> f64 + Send + Sync>;

/// A laid-out diagram, ready to rasterise.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagram {
    pub svg: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiagramError {
    #[error("the diagram is longer than {MAX_SOURCE} bytes")]
    TooLong,
    #[error("the diagram is too large to draw")]
    TooLarge,
    #[error("this diagram type is not supported")]
    Unsupported,
    #[error("{0}")]
    Parse(String),
    #[error("the diagram took too long to draw")]
    Timeout,
    #[error("{0}")]
    Failed(String),
}

/// The keyword a source starts with, past a byte-order mark, front matter,
/// `%%` comments and blank lines.
pub fn diagram_type(source: &str) -> Option<&str> {
    // Mermaid skips a byte-order mark, as a pasted file may start with one.
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let mut lines = source.lines().map(str::trim);
    let mut in_front_matter = false;
    let mut first = true;
    for line in lines.by_ref() {
        if first && line == "---" {
            in_front_matter = true;
            first = false;
            continue;
        }
        first = false;
        if in_front_matter {
            if line == "---" {
                in_front_matter = false;
            }
            continue;
        }
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        let end = line
            .find(|c: char| c.is_whitespace() || c == ';' || c == ':')
            .unwrap_or(line.len());
        return Some(&line[..end]);
    }
    None
}

/// Lay `source` out as an SVG in `theme`, measuring text with `measure`.
/// Blocking: run it off the main thread.
pub fn render(
    source: &str,
    theme: &DiagramTheme,
    measure: Measure,
) -> Result<Diagram, DiagramError> {
    render_with(source, theme, measure, OperationControl::new())
}

fn render_with(
    source: &str,
    theme: &DiagramTheme,
    measure: Measure,
    control: OperationControl,
) -> Result<Diagram, DiagramError> {
    if source.len() > MAX_SOURCE {
        return Err(DiagramError::TooLong);
    }
    let kind = diagram_type(source).ok_or(DiagramError::Unsupported)?;
    if !SUPPORTED.contains(&kind) {
        return Err(DiagramError::Unsupported);
    }
    let measurer = DeterministicTextMeasurer::default().with_width_callback(move |text, style| {
        let bold = style
            .font_weight
            .as_deref()
            .is_some_and(|w| w == "bold" || w.parse::<u32>().is_ok_and(|w| w >= 600));
        measure(text, style.font_size, bold)
    });
    let identity = TextMeasurementProfileIdentity::new(
        MeasurementProfileId::new("ircord.app-font").map_err(failed)?,
        format!("1:{}", theme.font),
    )
    .map_err(failed)?;
    let policy = TextMeasurementPolicy::uniform(TextMeasurementProfile::new(identity, measurer));
    let renderer = Renderer::new()
        .with_presentation_engine(MermaidEngine::new().with_site_config(site_config(theme)))
        .with_parse_options(ParseOptions::strict());
    let request = SvgRequest {
        environment: SvgEnvironment::deterministic()
            .with_resource_policy(RenderResourcePolicy::interactive())
            .with_text_measurement_policy(policy),
        pipeline: Some(SvgPipeline::resvg_safe()),
        ..SvgRequest::default()
    };
    let control = control.with_deadline(DEADLINE);
    match renderer.render(RenderRequest::svg(source, control, request)) {
        Ok(RenderOutput::Svg(Some(svg))) => Ok(Diagram {
            svg: strip_references(svg.svg()),
        }),
        Ok(_) | Err(RenderError::NoDiagram) => Err(DiagramError::Unsupported),
        Err(RenderError::Cancelled(c)) if c.reason == CancelReason::DeadlineExceeded => {
            Err(DiagramError::Timeout)
        }
        Err(RenderError::Parse(e)) => Err(DiagramError::Parse(e.to_string())),
        Err(RenderError::ResourceLimitExceeded(_)) => Err(DiagramError::TooLarge),
        Err(e) => Err(failed(e)),
    }
}

/// `svg` without any `href` that leaves the document: a `click` link
/// comes out as an `<a>` to its address, and an image node may carry one.
/// Only `#fragment` references (markers, gradients) stay. The attribute may
/// carry any namespace prefix (`xlink:href`, `xl:href`) and whitespace
/// around its `=`, as XML allows.
fn strip_references(svg: &str) -> String {
    let is_name = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.');
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(at) = rest.find("href") {
        let before = &rest[..at];
        // A prefix such as `xlink:` belongs to the attribute's name.
        let name_start = match before.strip_suffix(':') {
            Some(head) => head.trim_end_matches(is_name).len(),
            None => at,
        };
        let after = &rest[at + "href".len()..];
        let value = after
            .trim_start()
            .strip_prefix('=')
            .map(str::trim_start)
            .filter(|v| v.starts_with(['"', '\'']));
        let (true, Some(value)) = (rest[..name_start].ends_with(char::is_whitespace), value) else {
            out.push_str(&rest[..at + "href".len()]);
            rest = after;
            continue;
        };
        let value_start = rest.len() - value.len();
        let quote = value.chars().next().unwrap_or('"');
        let Some(len) = value[1..].find(quote) else {
            // An unterminated value: nothing after it is kept.
            out.push_str(rest[..name_start].trim_end());
            return out;
        };
        let end = value_start + 1 + len + 1;
        if value[1..].starts_with('#') {
            out.push_str(&rest[..end]);
        } else {
            out.push_str(rest[..name_start].trim_end());
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn failed(e: impl std::fmt::Display) -> DiagramError {
    DiagramError::Failed(e.to_string())
}

/// The app's colours as Mermaid's `base` theme, which every family reads.
fn site_config(t: &DiagramTheme) -> SourcePresentationConfig {
    let family = format!("\"{}\", sans-serif", t.font.replace(['"', '\\'], ""));
    let colours: &[(&str, &String)] = &[
        ("background", &t.background),
        ("mainBkg", &t.node),
        ("primaryColor", &t.node),
        ("primaryTextColor", &t.text),
        ("primaryBorderColor", &t.border),
        ("secondaryColor", &t.secondary),
        ("secondaryTextColor", &t.text),
        ("secondaryBorderColor", &t.border),
        ("tertiaryColor", &t.tertiary),
        ("tertiaryTextColor", &t.text),
        ("tertiaryBorderColor", &t.border),
        ("lineColor", &t.line),
        ("textColor", &t.text),
        ("titleColor", &t.text),
        ("edgeLabelBackground", &t.background),
        ("noteBkgColor", &t.secondary),
        ("noteTextColor", &t.text),
        ("noteBorderColor", &t.border),
        ("actorBkg", &t.node),
        ("actorBorder", &t.border),
        ("actorTextColor", &t.text),
        ("signalColor", &t.line),
        ("signalTextColor", &t.text),
        ("labelTextColor", &t.text),
        ("loopTextColor", &t.text),
        ("sectionBkgColor", &t.secondary),
        ("taskTextColor", &t.text),
        ("pieTitleTextColor", &t.text),
        ("pieSectionTextColor", &t.text),
        ("pieLegendTextColor", &t.text),
        ("pieStrokeColor", &t.border),
        ("pieOuterStrokeColor", &t.border),
        ("git0", &t.line),
        ("gitBranchLabel0", &t.text),
        ("quadrantTitleFill", &t.text),
        ("quadrantPointTextFill", &t.text),
    ];
    let mut variables: serde_json::Map<String, serde_json::Value> = colours
        .iter()
        .map(|(k, v)| (k.to_string(), serde_json::Value::from(v.as_str())))
        .collect();
    variables.insert("darkMode".into(), t.dark.into());
    variables.insert("fontFamily".into(), family.clone().into());
    variables.insert("fontSize".into(), "14px".into());
    SourcePresentationConfig::from_value(serde_json::json!({
        "theme": "base",
        "fontFamily": family,
        "fontSize": "14px",
        "darkMode": t.dark,
        "themeVariables": variables,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> DiagramTheme {
        DiagramTheme {
            dark: true,
            font: "Inter".into(),
            text: "#dbdee1".into(),
            node: "#2b2d31".into(),
            border: "#4e5058".into(),
            line: "#b5bac1".into(),
            secondary: "#232428".into(),
            tertiary: "#1e1f22".into(),
            background: "#313338".into(),
        }
    }

    /// A rough measure: every character half an em.
    fn measure() -> Measure {
        Arc::new(|text, size, _| text.chars().count() as f64 * size * 0.5)
    }

    fn draw(source: &str) -> Result<Diagram, DiagramError> {
        render(source, &theme(), measure())
    }

    const FAMILIES: &[(&str, &str)] = &[
        (
            "flowchart",
            "flowchart TD\n  A[Start] -->|go| B{Choice}\n  B -->|yes| C[Done]\n  B -->|no| A",
        ),
        (
            "sequence",
            "sequenceDiagram\n  Alice->>Bob: Hello\n  Bob-->>Alice: Hi\n  Note over Alice,Bob: a note",
        ),
        (
            "class",
            "classDiagram\n  Animal <|-- Duck\n  Animal : +int age\n  Duck : +swim()",
        ),
        (
            "state",
            "stateDiagram-v2\n  [*] --> Still\n  Still --> Moving\n  Moving --> [*]",
        ),
        (
            "er",
            "erDiagram\n  CUSTOMER ||--o{ ORDER : places\n  ORDER ||--|{ LINE : contains",
        ),
        (
            "gantt",
            "gantt\n  title Plan\n  dateFormat YYYY-MM-DD\n  section A\n  Task one :a1, 2026-01-01, 3d\n  Task two :after a1, 2d",
        ),
        (
            "pie",
            "pie title Pets\n  \"Dogs\" : 40\n  \"Cats\" : 35\n  \"Fish\" : 25",
        ),
        (
            "gitgraph",
            "gitGraph\n  commit\n  branch dev\n  checkout dev\n  commit\n  checkout main\n  merge dev",
        ),
        (
            "mindmap",
            "mindmap\n  root((IRCord))\n    Web\n    GPUI\n      Diagrams",
        ),
        (
            "timeline",
            "timeline\n  title History\n  2024 : Started\n  2025 : Web client\n  2026 : GPUI client",
        ),
        (
            "quadrant",
            "quadrantChart\n  title Reach\n  x-axis Low --> High\n  y-axis Low --> High\n  quadrant-1 Expand\n  quadrant-2 Promote\n  quadrant-3 Re-evaluate\n  quadrant-4 Improve\n  A: [0.3, 0.6]\n  B: [0.45, 0.23]",
        ),
        (
            "xychart",
            "xychart-beta\n  title Sales\n  x-axis [jan, feb, mar]\n  y-axis \"Revenue\" 0 --> 100\n  bar [30, 60, 90]\n  line [30, 60, 90]",
        ),
        (
            "journey",
            "journey\n  title My day\n  section Work\n    Write code: 5: Me\n    Review: 3: Me, Cat",
        ),
    ];

    #[test]
    fn every_family_draws_a_safe_svg() {
        for (name, source) in FAMILIES {
            let svg = draw(source).unwrap_or_else(|e| panic!("{name}: {e}")).svg;
            assert!(
                svg.starts_with("<svg") || svg.contains("<svg"),
                "{name}: no svg"
            );
            assert!(!svg.contains("<foreignObject"), "{name}: foreignObject");
            assert!(!svg.contains("<script"), "{name}: script");
        }
    }

    #[test]
    fn errors_are_told_apart() {
        assert!(matches!(
            draw("flowchart TD\n  A[unterminated --> B"),
            Err(DiagramError::Parse(_))
        ));
        assert_eq!(
            draw("sankey-beta\n  a,b,10"),
            Err(DiagramError::Unsupported)
        );
        assert_eq!(draw("just some text"), Err(DiagramError::Unsupported));
        assert_eq!(draw(""), Err(DiagramError::Unsupported));
        let long = format!("flowchart TD\n{}", "  A --> B\n".repeat(900));
        assert!(long.len() > 9000);
        assert_eq!(draw(&long), Err(DiagramError::TooLong));
    }

    #[test]
    fn a_render_past_its_deadline_is_a_timeout() {
        let control = OperationControl::new().with_deadline(Duration::ZERO);
        let r = render_with("flowchart TD\n  A --> B", &theme(), measure(), control);
        assert_eq!(r, Err(DiagramError::Timeout));
    }

    #[test]
    fn the_theme_colours_and_bold_labels_reach_the_layout() {
        let svg = draw(FAMILIES[0].1).unwrap().svg.to_ascii_lowercase();
        let t = theme();
        assert!(
            svg.contains(&t.text),
            "the text colour {} is not in the drawing",
            t.text
        );
        assert!(
            svg.contains(&t.node),
            "the node colour {} is not in the drawing",
            t.node
        );
        // Some family measures a bold label (titles, class names); the
        // measure must be told so.
        let bold = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = bold.clone();
        let measure: Measure = Arc::new(move |text, size, is_bold| {
            if is_bold {
                seen.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            text.chars().count() as f64 * size * 0.5
        });
        for (_, source) in FAMILIES {
            let _ = render(source, &t, measure.clone());
        }
        assert!(
            bold.load(std::sync::atomic::Ordering::Relaxed),
            "no label was measured bold"
        );
    }

    #[test]
    fn a_cancelled_render_is_not_a_timeout() {
        let control = OperationControl::new();
        control.cancel();
        let r = render_with("flowchart TD\n  A --> B", &theme(), measure(), control);
        assert!(matches!(r, Err(DiagramError::Failed(_))), "{r:?}");
    }

    #[test]
    fn the_type_skips_front_matter_and_comments() {
        assert_eq!(
            diagram_type("---\ntitle: x\n---\n%% hi\n\ngraph LR; A-->B"),
            Some("graph")
        );
        assert_eq!(diagram_type("  pie title Pets"), Some("pie"));
        assert_eq!(diagram_type("\u{feff}flowchart TD"), Some("flowchart"));
        assert!(draw("\u{feff}flowchart TD\n  A --> B").is_ok());
        assert!(draw("flowchart-elk LR\n  A --> B").is_ok());
        assert_eq!(diagram_type("\n\n"), None);
        assert!(is_diagram(Some("Mermaid")));
        assert!(!is_diagram(Some("rust")));
        assert!(!is_diagram(None));
    }

    /// Nothing in a diagram may reach the network or the disk: images,
    /// style sheets, links and `init` overrides all come out inert.
    #[test]
    fn hostile_sources_leave_no_outside_reference() {
        let hostile = [
            "flowchart TD\n  A@{ img: \"https://evil.example/a.png\", label: \"x\" }",
            "flowchart TD\n  A[<img src='https://evil.example/b.png'>] --> B",
            "flowchart TD\n  A --> B\n  click A \"https://evil.example/c\" \"tip\"\n  click B href \"javascript:alert(1)\"",
            "%%{init: {\"themeCSS\": \".node rect { fill: url(https://evil.example/d.svg#x); } @import url('https://evil.example/e.css');\"}}%%\nflowchart TD\n  A --> B",
            "%%{init: {\"fontFamily\": \"x'); background: url(https://evil.example/f\"}}%%\nflowchart TD\n  A --> B",
            "flowchart TD\n  A[\"<a href='https://evil.example/g'>link</a>\"] --> B\n  style A fill:url(https://evil.example/h)",
            "flowchart TD\n  classDef k fill:url(http://evil.example/i)\n  A:::k --> B",
            "sequenceDiagram\n  link Alice: Site @ https://evil.example/j\n  Alice->>Bob: hi",
            "flowchart TD\n  A[\"<image href='file:///etc/passwd'/>\"] --> B",
        ];
        let mut drawn = 0;
        for source in hostile {
            let Ok(d) = draw(source) else {
                continue;
            };
            drawn += 1;
            let svg = d.svg.to_ascii_lowercase();
            for bad in [
                "href=\"http",
                "href='http",
                "url(http",
                "url('http",
                "url(\"http",
                "@import",
                "javascript:",
                "file:",
            ] {
                assert!(!svg.contains(bad), "{bad} in the drawing of {source:?}");
            }
            assert!(
                !image_has_source(&svg),
                "an image source in the drawing of {source:?}"
            );
            assert!(
                !svg.contains("<foreignobject"),
                "foreignObject from {source:?}"
            );
            assert!(!svg.contains("<script"), "script from {source:?}");
        }
        // Each one must actually be drawn, or the checks above check nothing.
        assert_eq!(drawn, hostile.len());
    }

    #[test]
    fn outside_references_are_dropped_and_fragments_kept() {
        let svg = r##"<svg><a xlink:href="https://x.example/a"><use href="#m"/></a><image href='data:image/png;base64,AA'/></svg>"##;
        assert_eq!(
            strip_references(svg),
            r##"<svg><a><use href="#m"/></a><image/></svg>"##
        );
        assert_eq!(strip_references("no links"), "no links");
        // Any namespace prefix, and whitespace around `=`.
        let svg = r##"<svg><image xl:href="file:///x"/><image xlink:href = "file:///y"/><a href ='https://z'>t</a><use xlink:href = "#k"/></svg>"##;
        assert_eq!(
            strip_references(svg),
            r##"<svg><image/><image/><a>t</a><use xlink:href = "#k"/></svg>"##
        );
    }

    /// Whether any `<image …>` element carries an `href`.
    fn image_has_source(svg: &str) -> bool {
        svg.match_indices("<image").any(|(i, _)| {
            let tag = &svg[i..svg[i..].find('>').map_or(svg.len(), |e| i + e)];
            tag.contains("href=")
        })
    }
}
