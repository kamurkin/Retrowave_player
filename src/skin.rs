use egui::{Color32, Context, FontFamily, FontId, Stroke, Style, TextStyle, Theme, Vec2, Visuals};

pub const FRAME: Color32 = Color32::from_rgb(35, 36, 37);
pub const PANEL: Color32 = Color32::from_rgb(18, 19, 20);
pub const RAISED: Color32 = Color32::from_rgb(77, 78, 79);
pub const TEXT: Color32 = Color32::from_rgb(196, 198, 192);
pub const LCD: Color32 = Color32::from_rgb(145, 255, 67);
pub const MUTED: Color32 = Color32::from_rgb(113, 118, 107);
pub const GOLD: Color32 = Color32::from_rgb(227, 173, 65);

pub fn apply(context: &Context) {
    let mut style = Style::default();
    style.visuals = Visuals::dark();
    style.visuals.panel_fill = FRAME;
    style.visuals.window_fill = FRAME;
    style.visuals.extreme_bg_color = PANEL;
    style.visuals.widgets.noninteractive.bg_fill = FRAME;
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(56, 57, 58);
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(88, 90, 90);
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(30, 75, 18);
    style.visuals.widgets.active.fg_stroke = Stroke::new(1.0, LCD);
    style.visuals.selection.bg_fill = Color32::from_rgb(43, 82, 29);
    style.visuals.window_corner_radius = 0.into();
    style.spacing.item_spacing = Vec2::new(5.0, 4.0);
    style.spacing.button_padding = Vec2::new(6.0, 3.0);
    style
        .text_styles
        .insert(TextStyle::Body, FontId::new(12.0, FontFamily::Monospace));
    style
        .text_styles
        .insert(TextStyle::Button, FontId::new(11.0, FontFamily::Monospace));
    style
        .text_styles
        .insert(TextStyle::Heading, FontId::new(13.0, FontFamily::Monospace));
    context.set_style_of(Theme::Dark, style);
    context.set_theme(Theme::Dark);
}
