use super::fonts;
use eframe::egui::{self, Color32};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum Theme {
    Glass,
    #[default]
    #[serde(other)]
    Ink,
}

impl Theme {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ink => "Ink",
            Self::Glass => "Glass",
        }
    }
}

pub(super) fn current(ctx: &egui::Context) -> Theme {
    ctx.data(|d| d.get_temp::<Theme>(egui::Id::unique("native-theme")))
        .unwrap_or_default()
}

/// The interface's colours, after MonoCode: near-black surfaces, grey text and one blue
/// accent, with colour kept for state so that a hop that is fine stays quiet.
#[derive(Clone, Copy)]
pub(super) struct Palette {
    /// Black, for marks and glyphs drawn on a filled colour.
    pub surface: Color32,
    /// MonoCode's shell: the title bar, tabs, toolbar and settings navigation.
    pub shell: Color32,
    /// MonoCode's inset pane, a step darker: the table, graphs and settings pages.
    pub inset: Color32,
    pub raised: Color32,
    pub input: Color32,
    pub line: Color32,
    /// The edge of a control: 3:1 against the shell, as WCAG asks of control boundaries.
    pub edge: Color32,
    pub text: Color32,
    pub soft: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub accent_light: Color32,
    pub good: Color32,
    pub warn: Color32,
    pub bad: Color32,
}
const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}
/// MonoCode's Ink theme.
// Ink's greys come from MonoCode's default neutral shell, hsl(240, 0%, 9%), and its inset
// pane at 45% of that lightness.
const INK: Palette = Palette {
    surface: rgb(0x000000),
    shell: rgb(0x171717),
    inset: rgb(0x0a0a0a),
    raised: rgb(0x1f1f1f),
    input: rgb(0x0f0f0f),
    line: rgb(0x2a2a2a),
    edge: rgb(0x646464),
    text: rgb(0xf5f5f5),
    soft: rgb(0xd6d6d6),
    muted: rgb(0x8f8f8f),
    accent: rgb(0x7aa7ff),
    accent_light: rgb(0x9cc0ff),
    good: rgb(0x8fd6a8),
    warn: rgb(0xf2c46d),
    bad: rgb(0xff6b6b),
};

const GLASS: Palette = Palette {
    surface: rgb(0x101410),
    shell: rgb(0x292e33),
    inset: rgb(0x16191c),
    raised: rgb(0x22262a),
    input: rgb(0x1b1e22),
    line: rgb(0x3e454b),
    edge: rgb(0x7d8792),
    text: rgb(0xebeef2),
    soft: rgb(0xc6cbd1),
    muted: rgb(0xa6adb6),
    accent: rgb(0x76b82a),
    accent_light: rgb(0xa4d65e),
    good: rgb(0x8fd16a),
    warn: rgb(0xf2c46d),
    bad: rgb(0xff7777),
};

/// The colours the interface paints with.
pub(super) fn palette(ctx: &egui::Context) -> &'static Palette {
    match current(ctx) {
        Theme::Ink => &INK,
        Theme::Glass => &GLASS,
    }
}
pub(super) fn apply_theme(ctx: &egui::Context, theme: Theme) {
    let previous = ctx.data(|d| d.get_temp::<Theme>(egui::Id::unique("native-theme")));
    if previous == Some(theme) {
        return;
    }
    if previous.is_none() {
        ctx.set_fonts(fonts().clone());
    }
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("native-theme"), theme));
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = palette(ctx).shell;
    visuals.window_fill = palette(ctx).raised;
    visuals.extreme_bg_color = palette(ctx).input;
    visuals.faint_bg_color = palette(ctx).raised;
    visuals.window_stroke = egui::Stroke::new(1.0, palette(ctx).line);
    visuals.hyperlink_color = palette(ctx).accent_light;
    visuals.warn_fg_color = palette(ctx).warn;
    visuals.error_fg_color = palette(ctx).bad;
    visuals.selection.bg_fill = palette(ctx).accent.gamma_multiply(0.3);
    visuals.selection.stroke = egui::Stroke::new(1.0, palette(ctx).text);
    let w = &mut visuals.widgets;
    w.noninteractive.bg_fill = palette(ctx).shell;
    w.noninteractive.weak_bg_fill = palette(ctx).shell;
    w.noninteractive.bg_stroke = egui::Stroke::new(1.0, palette(ctx).line);
    w.noninteractive.fg_stroke = egui::Stroke::new(1.0, palette(ctx).soft);
    // Controls are lifted by translucent ink, not by a tint of their own.
    for (state, fill, text) in [
        (&mut w.inactive, 0.08, palette(ctx).soft),
        (&mut w.hovered, 0.14, palette(ctx).text),
        (&mut w.active, 0.2, palette(ctx).text),
        (&mut w.open, 0.14, palette(ctx).text),
    ] {
        state.bg_fill = palette(ctx).text.gamma_multiply(fill);
        state.weak_bg_fill = state.bg_fill;
        state.fg_stroke = egui::Stroke::new(1.0, text);
    }
    w.inactive.bg_stroke = egui::Stroke::new(1.0, palette(ctx).edge);
    w.hovered.bg_stroke = egui::Stroke::new(1.0, palette(ctx).muted);
    if theme == Theme::Glass {
        visuals.window_corner_radius = egui::CornerRadius::same(12);
        visuals.menu_corner_radius = egui::CornerRadius::same(8);
        visuals.slider_trailing_fill = true;
        visuals.handle_shape = egui::style::HandleShape::Circle;
        for state in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            state.corner_radius = egui::CornerRadius::same(6);
        }
        w.active.bg_stroke = egui::Stroke::new(1.0, palette(ctx).accent);
        w.open.bg_stroke = egui::Stroke::new(1.0, palette(ctx).accent);
    }
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_visuals(visuals);
    ctx.style_mut_of(egui::Theme::Dark, |s| {
        s.spacing.item_spacing = egui::vec2(8.0, 5.0);
        s.spacing.interact_size.y = 32.0;
        s.spacing.slider_rail_height = if theme == Theme::Glass { 3.0 } else { 8.0 };
        s.text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
        s.text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
        s.text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
        s.text_styles
            .insert(egui::TextStyle::Monospace, egui::FontId::monospace(13.0));
    });
}

/// A quiet light across the charcoal surface gives the cards depth without desktop blur.
pub(super) fn glass_surface(ui: &egui::Ui) {
    if current(ui.ctx()) != Theme::Glass {
        return;
    }
    let rect = ui.max_rect();
    let mut mesh = egui::Mesh::default();
    for (pos, color) in [
        (rect.left_top(), rgb(0x30363b)),
        (rect.right_top(), rgb(0x24292d)),
        (rect.left_bottom(), rgb(0x16191c)),
        (rect.right_bottom(), rgb(0x1b2023)),
    ] {
        mesh.colored_vertex(pos, color);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    ui.painter().add(egui::Shape::mesh(mesh));
    ui.painter().hline(
        rect.x_range(),
        rect.top(),
        egui::Stroke::new(1.0, palette(ui.ctx()).line),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes_are_isolated_per_window_and_switch_back_completely() {
        let glass = egui::Context::default();
        let ink = egui::Context::default();
        apply_theme(&glass, Theme::Glass);
        apply_theme(&ink, Theme::Ink);
        assert_eq!(palette(&glass).accent, GLASS.accent);
        assert_eq!(palette(&ink).accent, INK.accent);
        assert!(
            glass
                .style_of(egui::Theme::Dark)
                .visuals
                .slider_trailing_fill
        );

        apply_theme(&glass, Theme::Ink);
        assert_eq!(palette(&glass).accent, INK.accent);
        let switched = glass.style_of(egui::Theme::Dark);
        let original = ink.style_of(egui::Theme::Dark);
        assert_eq!(switched.visuals, original.visuals);
        assert_eq!(switched.spacing, original.spacing);
    }
}
