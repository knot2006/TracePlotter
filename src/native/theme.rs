use eframe::egui::{self, Color32};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum Theme {
    #[serde(alias = "ink")]
    Dark,
    #[default]
    #[serde(other)]
    Glass,
}

impl Theme {
    pub fn is_glass(self) -> bool {
        self == Self::Glass
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Dark => "Dark",
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
/// MonoCode's neutral dark theme.
// Dark's greys come from MonoCode's default neutral shell, hsl(240, 0%, 9%), and its inset
// pane at 45% of that lightness.
const DARK: Palette = Palette {
    surface: rgb(0x000000),
    shell: rgb(0x171717),
    inset: rgb(0x0a0a0a),
    raised: rgb(0x1f1f1f),
    input: rgb(0x0f0f0f),
    line: rgb(0x2a2a2a),
    edge: rgb(0x646464),
    text: rgb(0xebebeb),
    soft: rgb(0xb6b6b6),
    muted: rgb(0x969696),
    accent: rgb(0x4da3f5),
    accent_light: rgb(0x79b9f8),
    good: rgb(0x3ddc84),
    warn: rgb(0xffb020),
    bad: rgb(0xff4d4d),
};

const GLASS: Palette = Palette {
    surface: rgb(0x101410),
    shell: rgb(0x2a2e33),
    inset: rgb(0x1c1f23),
    raised: rgb(0x23272b),
    input: rgb(0x1a1d21),
    line: rgb(0x353b41),
    edge: rgb(0x7d8792),
    text: rgb(0xeef0f2),
    soft: rgb(0xd2d6db),
    muted: rgb(0x9aa1a9),
    accent: rgb(0x76b900),
    accent_light: rgb(0x9bd442),
    good: rgb(0x8fd16a),
    warn: rgb(0xf2c46d),
    bad: rgb(0xff7777),
};

/// The colours the interface paints with.
pub(super) fn palette(ctx: &egui::Context) -> &'static Palette {
    match current(ctx) {
        Theme::Dark => &DARK,
        Theme::Glass => &GLASS,
    }
}
pub(super) fn apply_theme(ctx: &egui::Context, theme: Theme) {
    let previous = ctx.data(|d| d.get_temp::<Theme>(egui::Id::unique("native-theme")));
    if previous == Some(theme) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("native-theme"), theme));
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = palette(ctx).shell.gamma_multiply(opacity(ctx));
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
    if theme == Theme::Dark {
        visuals.window_corner_radius = egui::CornerRadius::same(12);
        visuals.menu_corner_radius = egui::CornerRadius::same(8);
        visuals.slider_trailing_fill = true;
        visuals.handle_shape = egui::style::HandleShape::Circle;
        visuals.selection.bg_fill = palette(ctx).text.gamma_multiply(0.10);
        for (state, fill, edge) in [
            (&mut w.inactive, 0.04, 0.10),
            (&mut w.hovered, 0.10, 0.20),
            (&mut w.active, 0.14, 0.20),
            (&mut w.open, 0.10, 0.20),
        ] {
            state.corner_radius = egui::CornerRadius::same(6);
            state.bg_fill = palette(ctx).text.gamma_multiply(fill);
            state.weak_bg_fill = state.bg_fill;
            state.bg_stroke = egui::Stroke::new(1.0, palette(ctx).text.gamma_multiply(edge));
        }
        // The accent stays on the control in use, including keyboard focus.
        w.active.bg_stroke = egui::Stroke::new(1.0, palette(ctx).accent);
        w.open.bg_stroke = egui::Stroke::new(1.0, palette(ctx).accent);
    }
    if theme == Theme::Glass {
        visuals.window_corner_radius = egui::CornerRadius::same(12);
        visuals.menu_corner_radius = egui::CornerRadius::same(10);
        // Menus and dialogs are frosted panes of their own: a step lighter than the panels,
        // rimmed with light rather than a dark line, and lifted by a soft shadow.
        visuals.window_fill = rgb(0x2b3035);
        visuals.window_stroke = egui::Stroke::new(1.0, palette(ctx).text.gamma_multiply(0.12));
        visuals.popup_shadow = egui::Shadow {
            offset: [0, 6],
            blur: 18,
            spread: 0,
            color: Color32::from_black_alpha(120),
        };
        visuals.slider_trailing_fill = true;
        visuals.handle_shape = egui::style::HandleShape::Circle;
        // A chosen option is an olive plate with green lettering, as the segmented controls
        // of graphics drivers' overlays have.
        visuals.selection.bg_fill = palette(ctx).accent.gamma_multiply(0.28);
        visuals.selection.stroke = egui::Stroke::new(1.0, palette(ctx).accent_light);
        // Controls are soft plates without outlines; only the one in use takes the accent.
        for (state, fill, edge) in [
            (&mut w.inactive, 0.09, 0.05),
            (&mut w.hovered, 0.15, 0.12),
            (&mut w.active, 0.2, 0.0),
            (&mut w.open, 0.15, 0.0),
        ] {
            state.corner_radius = egui::CornerRadius::same(8);
            state.bg_fill = palette(ctx).text.gamma_multiply(fill);
            state.weak_bg_fill = state.bg_fill;
            state.bg_stroke = egui::Stroke::new(1.0, palette(ctx).text.gamma_multiply(edge));
        }
        w.active.bg_stroke = egui::Stroke::new(1.0, palette(ctx).accent);
        w.open.bg_stroke = egui::Stroke::new(1.0, palette(ctx).accent);
        visuals.window_shadow = egui::Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(110),
        };
    }
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_visuals(visuals);
    ctx.style_mut_of(egui::Theme::Dark, |s| {
        s.spacing.item_spacing = egui::vec2(8.0, 5.0);
        s.spacing.interact_size.y = match theme {
            Theme::Glass => 30.0,
            Theme::Dark => 28.0,
        };
        s.spacing.slider_rail_height = 4.0;
        s.spacing.menu_margin = egui::Margin::same(if theme.is_glass() { 8 } else { 6 });
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

/// How opaque the window's panels are. Dark stays solid outside Windows.
pub(super) fn opacity(ctx: &egui::Context) -> f32 {
    let theme = current(ctx);
    if theme == Theme::Dark && !cfg!(windows) {
        return 1.0;
    }
    let transparency = ctx
        .data(|d| d.get_temp::<f32>(transparency_id(theme)))
        .unwrap_or(0.0);
    1.0 - transparency
}
pub(super) fn set_transparency(ctx: &egui::Context, transparency: f32) {
    // A saved session is not trusted to hold a usable number.
    let transparency = if transparency.is_finite() {
        transparency.clamp(0.0, 0.7)
    } else {
        0.0
    };
    let id = transparency_id(current(ctx));
    ctx.data_mut(|d| d.insert_temp(id, transparency));
    let fill = palette(ctx).shell.gamma_multiply(opacity(ctx));
    ctx.style_mut_of(egui::Theme::Dark, |s| s.visuals.panel_fill = fill);
}

fn transparency_id(theme: Theme) -> egui::Id {
    egui::Id::unique(match theme {
        Theme::Dark => "native-dark-transparency",
        Theme::Glass => "native-transparency",
    })
}

pub(super) fn tab_height(ctx: &egui::Context) -> f32 {
    match current(ctx) {
        Theme::Dark => 27.0,
        Theme::Glass => 28.0,
    }
}

/// Cool blue light gives Glass depth even where the desktop compositor cannot blur it.
pub(super) fn glass_surface(ui: &egui::Ui, margin: egui::Margin) {
    if !current(ui.ctx()).is_glass() {
        return;
    }
    let rect = egui::Rect::from_min_max(
        ui.max_rect().min - egui::vec2(f32::from(margin.left), f32::from(margin.top)),
        ui.max_rect().max + egui::vec2(f32::from(margin.right), f32::from(margin.bottom)),
    );
    let mut mesh = egui::Mesh::default();
    let colours = [
        (rect.left_top(), rgb(0x2c3136)),
        (rect.right_top(), rgb(0x23272b)),
        (rect.left_bottom(), rgb(0x181b1e)),
        (rect.right_bottom(), rgb(0x1d2125)),
    ];
    for (pos, color) in colours {
        mesh.colored_vertex(pos, color.gamma_multiply(opacity(ui.ctx())));
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
        let dark = egui::Context::default();
        apply_theme(&glass, Theme::Glass);
        apply_theme(&dark, Theme::Dark);
        assert_eq!(palette(&glass).accent, GLASS.accent);
        assert_eq!(palette(&dark).accent, DARK.accent);
        assert!(
            glass
                .style_of(egui::Theme::Dark)
                .visuals
                .slider_trailing_fill
        );

        apply_theme(&glass, Theme::Dark);
        assert_eq!(palette(&glass).accent, DARK.accent);
        let switched = glass.style_of(egui::Theme::Dark);
        let original = dark.style_of(egui::Theme::Dark);
        assert_eq!(switched.visuals, original.visuals);
        assert_eq!(switched.spacing, original.spacing);
    }

    #[test]
    fn transparency_is_bounded_and_independent_for_each_theme() {
        let ctx = egui::Context::default();
        apply_theme(&ctx, Theme::Glass);
        set_transparency(&ctx, 5.0);
        assert_eq!(opacity(&ctx), 0.3);
        assert!(ctx.style_of(egui::Theme::Dark).visuals.panel_fill.a() < 255);
        set_transparency(&ctx, f32::NAN);
        assert_eq!(opacity(&ctx), 1.0);
        set_transparency(&ctx, 0.5);
        apply_theme(&ctx, Theme::Dark);
        assert_eq!(opacity(&ctx), 1.0);
        assert_eq!(
            ctx.style_of(egui::Theme::Dark).visuals.panel_fill,
            DARK.shell
        );
        set_transparency(&ctx, 0.15);
        assert_eq!(opacity(&ctx), if cfg!(windows) { 0.85 } else { 1.0 });
        apply_theme(&ctx, Theme::Glass);
        assert_eq!(opacity(&ctx), 0.5);
    }
}
