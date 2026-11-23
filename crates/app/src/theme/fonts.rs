//! The bundled fonts (`crates/app/assets/fonts`): every interface and message
//! font the web client offers, registered once at start so a family name in
//! the theme finds them on every platform.

use std::borrow::Cow;

use gpui_kit::App;

static FILES: &[&[u8]] = &[
    include_bytes!("../../assets/fonts/AtkinsonHyperlegibleMono-Bold.ttf"),
    include_bytes!("../../assets/fonts/AtkinsonHyperlegibleMono-Regular.ttf"),
    include_bytes!("../../assets/fonts/AtkinsonHyperlegibleMono-SemiBold.ttf"),
    include_bytes!("../../assets/fonts/AtkinsonHyperlegibleNext-Bold.ttf"),
    include_bytes!("../../assets/fonts/AtkinsonHyperlegibleNext-Regular.ttf"),
    include_bytes!("../../assets/fonts/AtkinsonHyperlegibleNext-SemiBold.ttf"),
    include_bytes!("../../assets/fonts/Geist-Bold.ttf"),
    include_bytes!("../../assets/fonts/Geist-Regular.ttf"),
    include_bytes!("../../assets/fonts/Geist-SemiBold.ttf"),
    include_bytes!("../../assets/fonts/GeistMono-Bold.ttf"),
    include_bytes!("../../assets/fonts/GeistMono-Regular.ttf"),
    include_bytes!("../../assets/fonts/GeistMono-SemiBold.ttf"),
    include_bytes!("../../assets/fonts/IBMPlexSans-Bold.ttf"),
    include_bytes!("../../assets/fonts/IBMPlexSans-Regular.ttf"),
    include_bytes!("../../assets/fonts/IBMPlexSans-SemiBold.ttf"),
    include_bytes!("../../assets/fonts/Inter-Bold.ttf"),
    include_bytes!("../../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../../assets/fonts/Inter-SemiBold.ttf"),
    include_bytes!("../../assets/fonts/JetBrainsMono-Bold.ttf"),
    include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../../assets/fonts/JetBrainsMono-SemiBold.ttf"),
    include_bytes!("../../assets/fonts/Lexend-Bold.ttf"),
    include_bytes!("../../assets/fonts/Lexend-Regular.ttf"),
    include_bytes!("../../assets/fonts/Lexend-SemiBold.ttf"),
    include_bytes!("../../assets/fonts/Literata-Bold.ttf"),
    include_bytes!("../../assets/fonts/Literata-Regular.ttf"),
    include_bytes!("../../assets/fonts/Literata-SemiBold.ttf"),
    include_bytes!("../../assets/fonts/Manrope-Bold.ttf"),
    include_bytes!("../../assets/fonts/Manrope-Regular.ttf"),
    include_bytes!("../../assets/fonts/Manrope-SemiBold.ttf"),
    include_bytes!("../../assets/fonts/Nunito-Bold.ttf"),
    include_bytes!("../../assets/fonts/Nunito-Regular.ttf"),
    include_bytes!("../../assets/fonts/Nunito-SemiBold.ttf"),
    include_bytes!("../../assets/fonts/SourceSans3-Bold.ttf"),
    include_bytes!("../../assets/fonts/SourceSans3-Regular.ttf"),
    include_bytes!("../../assets/fonts/SourceSans3-SemiBold.ttf"),
    include_bytes!("../../assets/fonts/SourceSerif4-Bold.ttf"),
    include_bytes!("../../assets/fonts/SourceSerif4-Regular.ttf"),
    include_bytes!("../../assets/fonts/SourceSerif4-SemiBold.ttf"),
];

/// The bundled font files, for drawing outside GPUI's text system
/// (diagrams, `message::diagram`).
pub fn files() -> &'static [&'static [u8]] {
    FILES
}

/// Register the bundled fonts with the text system.
pub fn register(cx: &mut App) {
    let fonts = FILES.iter().map(|bytes| Cow::Borrowed(*bytes)).collect();
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        log::warn!("could not load the bundled fonts: {e}");
    }
}
