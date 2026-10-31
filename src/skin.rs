use egui::{
    Align2, Color32, Context, CornerRadius, FontFamily, FontId, Painter, Rect, Response, Sense,
    Shape, Stroke, StrokeKind, TextStyle, Ui, Vec2, Visuals, pos2, vec2,
};
use std::ops::RangeInclusive;

pub const PANEL_BG: Color32 = Color32::from_rgb(43, 43, 43);
pub const PANEL_DARK: Color32 = Color32::from_rgb(28, 28, 28);
pub const BEVEL_LIGHT: Color32 = Color32::from_rgb(112, 112, 112);
pub const BEVEL_DARK: Color32 = Color32::from_rgb(12, 12, 12);
pub const BTN_FACE: Color32 = Color32::from_rgb(82, 82, 82);
pub const BTN_HOVER: Color32 = Color32::from_rgb(104, 104, 104);
pub const BTN_PRESSED: Color32 = Color32::from_rgb(58, 58, 58);
pub const TITLE_BG: Color32 = Color32::from_rgb(26, 30, 46);
pub const TITLE_BG2: Color32 = Color32::from_rgb(12, 14, 24);
pub const TITLE_TEXT: Color32 = Color32::from_rgb(196, 214, 230);
pub const ACCENT: Color32 = Color32::from_rgb(0, 214, 255);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(0, 92, 116);
pub const LCD_BG: Color32 = Color32::from_rgb(5, 18, 8);
pub const LCD_GREEN: Color32 = Color32::from_rgb(64, 255, 106);
pub const LCD_DIM: Color32 = Color32::from_rgb(28, 128, 52);
pub const LCD_YELLOW: Color32 = Color32::from_rgb(220, 255, 90);
pub const LCD_RED: Color32 = Color32::from_rgb(255, 90, 90);
pub const TEXT_DIM: Color32 = Color32::from_rgb(140, 140, 140);
pub const SELECT_BG: Color32 = Color32::from_rgb(24, 60, 84);

pub fn apply(context: &Context) {
    context.set_theme(egui::ThemePreference::Dark);
    let mut style = (*context.global_style()).clone();
    style.visuals = Visuals::dark();
    style.visuals.panel_fill = PANEL_BG;
    style.visuals.window_fill = PANEL_BG;
    style.visuals.extreme_bg_color = LCD_BG;
    style.visuals.faint_bg_color = PANEL_DARK;
    style.visuals.override_text_color = Some(TITLE_TEXT);
    style.visuals.selection.bg_fill = SELECT_BG;
    style.visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    style.visuals.widgets.noninteractive.bg_fill = PANEL_BG;
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TITLE_TEXT);
    style.visuals.widgets.inactive.bg_fill = BTN_FACE;
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TITLE_TEXT);
    style.visuals.widgets.hovered.bg_fill = BTN_HOVER;
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    style.visuals.widgets.active.bg_fill = BTN_PRESSED;
    style.visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    style.visuals.window_corner_radius = CornerRadius::ZERO;
    style.spacing.item_spacing = vec2(4.0, 3.0);
    style.spacing.button_padding = vec2(4.0, 2.0);
    style.spacing.interact_size = vec2(22.0, 16.0);
    style.spacing.slider_width = 100.0;
    style.animation_time = 0.05;
    style
        .text_styles
        .insert(TextStyle::Small, FontId::monospace(8.0));
    style
        .text_styles
        .insert(TextStyle::Body, FontId::new(10.0, FontFamily::Monospace));
    style
        .text_styles
        .insert(TextStyle::Button, FontId::monospace(9.0));
    style
        .text_styles
        .insert(TextStyle::Monospace, FontId::monospace(10.0));
    style
        .text_styles
        .insert(TextStyle::Heading, FontId::monospace(13.0));
    context.set_global_style(style);
}

fn lerp_color(a: Color32, b: Color32, amount: f32) -> Color32 {
    let amount = amount.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

pub fn bevel_rect(painter: &Painter, rect: Rect, fill: Color32, pressed: bool) {
    painter.rect_filled(rect, CornerRadius::ZERO, BEVEL_DARK);
    let inner = rect.shrink(1.0);
    painter.rect_filled(inner, CornerRadius::ZERO, fill);
    let (light, dark) = if pressed {
        (BEVEL_DARK, BEVEL_LIGHT)
    } else {
        (BEVEL_LIGHT, BEVEL_DARK)
    };
    painter.line_segment(
        [inner.left_top(), inner.right_top()],
        Stroke::new(1.0, light),
    );
    painter.line_segment(
        [inner.left_top(), inner.left_bottom()],
        Stroke::new(1.0, light),
    );
    painter.line_segment(
        [inner.left_bottom(), inner.right_bottom()],
        Stroke::new(1.0, dark),
    );
    painter.line_segment(
        [inner.right_top(), inner.right_bottom()],
        Stroke::new(1.0, dark),
    );
}

pub struct TitleBarResult {
    pub clicked: Option<usize>,
    pub drag_started: bool,
}

pub fn title_bar(ui: &mut Ui, title: &str, buttons: &[(&str, bool)]) -> TitleBarResult {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
    let painter = ui.painter();
    for step in 0..18 {
        let y0 = step as f32 / 18.0;
        let y1 = (step + 1) as f32 / 18.0;
        let strip = Rect::from_min_max(
            pos2(rect.left(), rect.top() + rect.height() * y0),
            pos2(rect.right(), rect.top() + rect.height() * y1),
        );
        painter.rect_filled(
            strip,
            CornerRadius::ZERO,
            lerp_color(TITLE_BG, TITLE_BG2, y0),
        );
    }
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, ACCENT_DIM),
    );
    painter.text(
        pos2(rect.left() + 6.0, rect.center().y),
        Align2::LEFT_CENTER,
        title,
        FontId::monospace(9.0),
        TITLE_TEXT,
    );

    let button_width = 17.0;
    let button_height = 12.0;
    let gap = 2.0;
    let buttons_left = rect.right() - 3.0 - buttons.len() as f32 * (button_width + gap) + gap;
    let mut clicked = None;
    for (index, (label, active)) in buttons.iter().enumerate() {
        let button_rect = Rect::from_min_size(
            pos2(
                buttons_left + index as f32 * (button_width + gap),
                rect.center().y - button_height / 2.0,
            ),
            vec2(button_width, button_height),
        );
        let response = ui.interact(
            button_rect,
            ui.scope_id().with(("title-button", title, index)),
            Sense::click(),
        );
        let pressed = response.is_pointer_button_down_on();
        let fill = if pressed {
            BTN_PRESSED
        } else if response.hovered() {
            BTN_HOVER
        } else if *active {
            ACCENT_DIM
        } else {
            BTN_FACE
        };
        bevel_rect(painter, button_rect, fill, pressed);
        painter.text(
            button_rect.center(),
            Align2::CENTER_CENTER,
            *label,
            FontId::monospace(7.5),
            if *active { Color32::WHITE } else { TITLE_TEXT },
        );
        if response.clicked() {
            clicked = Some(index);
        }
    }
    let drag_response = ui.interact(
        Rect::from_min_max(rect.min, pos2(buttons_left - 2.0, rect.max.y)),
        ui.scope_id().with(("title-drag", title)),
        Sense::click_and_drag(),
    );
    TitleBarResult {
        clicked,
        drag_started: drag_response.drag_started(),
    }
}

pub fn text_button(ui: &mut Ui, label: &str, size: Vec2, active: bool, enabled: bool) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let pressed = enabled && response.is_pointer_button_down_on();
    let fill = if !enabled {
        Color32::from_rgb(54, 54, 54)
    } else if pressed {
        BTN_PRESSED
    } else if response.hovered() {
        BTN_HOVER
    } else if active {
        ACCENT_DIM
    } else {
        BTN_FACE
    };
    bevel_rect(ui.painter(), rect, fill, pressed);
    let color = if !enabled {
        Color32::from_rgb(90, 90, 90)
    } else {
        TITLE_TEXT
    };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::monospace(8.0),
        color,
    );
    response
}

#[derive(Clone, Copy)]
pub enum TransportIcon {
    Previous,
    Play,
    Pause,
    Stop,
    Next,
}

pub fn transport_button(ui: &mut Ui, icon: TransportIcon, active: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(30.0, 22.0), Sense::click());
    let pressed = response.is_pointer_button_down_on();
    let fill = if pressed {
        BTN_PRESSED
    } else if response.hovered() {
        BTN_HOVER
    } else if active {
        ACCENT_DIM
    } else {
        BTN_FACE
    };
    bevel_rect(ui.painter(), rect, fill, pressed);
    draw_transport_icon(ui.painter(), rect.shrink(2.0), icon, Color32::WHITE);
    response
}

fn draw_transport_icon(painter: &Painter, rect: Rect, icon: TransportIcon, color: Color32) {
    let center = rect.center();
    match icon {
        TransportIcon::Play => {
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(center.x - 4.0, center.y - 6.0),
                    pos2(center.x - 4.0, center.y + 6.0),
                    pos2(center.x + 6.0, center.y),
                ],
                color,
                Stroke::NONE,
            ));
        }
        TransportIcon::Pause => {
            for x in [-3.0, 3.0] {
                painter.rect_filled(
                    Rect::from_center_size(pos2(center.x + x, center.y), vec2(3.0, 12.0)),
                    CornerRadius::ZERO,
                    color,
                );
            }
        }
        TransportIcon::Stop => {
            painter.rect_filled(
                Rect::from_center_size(center, vec2(9.0, 9.0)),
                CornerRadius::ZERO,
                color,
            );
        }
        TransportIcon::Previous | TransportIcon::Next => {
            let previous = matches!(icon, TransportIcon::Previous);
            let direction = if previous { -1.0 } else { 1.0 };
            painter.rect_filled(
                Rect::from_center_size(pos2(center.x - direction * 6.0, center.y), vec2(2.0, 12.0)),
                CornerRadius::ZERO,
                color,
            );
            let tip = center.x + direction * 5.0;
            let base = center.x - direction * 4.0;
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(tip, center.y),
                    pos2(base, center.y - 6.0),
                    pos2(base, center.y + 6.0),
                ],
                color,
                Stroke::NONE,
            ));
        }
    }
}

pub fn lcd_panel(ui: &mut Ui, size: Vec2) -> Rect {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, LCD_BG);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::ZERO,
        Stroke::new(1.0, BEVEL_DARK),
        StrokeKind::Inside,
    );
    ui.painter().line_segment(
        [
            rect.left_top() + vec2(1.0, 1.0),
            rect.right_top() + vec2(-1.0, 1.0),
        ],
        Stroke::new(1.0, Color32::from_rgb(18, 48, 24)),
    );
    rect
}

pub fn h_slider(ui: &mut Ui, value: &mut f32, range: RangeInclusive<f32>, width: f32) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(vec2(width.max(12.0), 12.0), Sense::click_and_drag());
    let min = *range.start();
    let max = *range.end();
    let track = Rect::from_min_max(
        pos2(rect.left() + 2.0, rect.center().y - 2.0),
        pos2(rect.right() - 2.0, rect.center().y + 2.0),
    );
    if max > min && (response.dragged() || response.clicked()) {
        if let Some(position) = response.interact_pointer_pos() {
            let t = ((position.x - track.left()) / track.width()).clamp(0.0, 1.0);
            *value = min + t * (max - min);
        }
    }
    ui.painter().rect_filled(track, CornerRadius::ZERO, LCD_BG);
    ui.painter().rect_stroke(
        track,
        CornerRadius::ZERO,
        Stroke::new(1.0, BEVEL_DARK),
        StrokeKind::Inside,
    );
    let t = if max > min {
        ((*value - min) / (max - min)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    if t > 0.0 {
        ui.painter().rect_filled(
            Rect::from_min_max(
                track.min,
                pos2(track.left() + track.width() * t, track.bottom()),
            ),
            CornerRadius::ZERO,
            LCD_DIM,
        );
    }
    let thumb = Rect::from_center_size(
        pos2(track.left() + track.width() * t, rect.center().y),
        vec2(7.0, 11.0),
    );
    bevel_rect(
        ui.painter(),
        thumb,
        BTN_FACE,
        response.is_pointer_button_down_on(),
    );
    response
}

pub fn v_slider(
    ui: &mut Ui,
    value: &mut f32,
    range: RangeInclusive<f32>,
    size: Vec2,
    enabled: bool,
) -> Response {
    let sense = if enabled {
        Sense::click_and_drag()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let min = *range.start();
    let max = *range.end();
    let track = Rect::from_center_size(rect.center(), vec2(4.0, (rect.height() - 4.0).max(1.0)));
    if enabled && max > min && (response.dragged() || response.clicked()) {
        if let Some(position) = response.interact_pointer_pos() {
            let t = ((track.bottom() - position.y) / track.height()).clamp(0.0, 1.0);
            *value = min + t * (max - min);
        }
    }
    ui.painter().rect_filled(track, CornerRadius::ZERO, LCD_BG);
    ui.painter().rect_stroke(
        track,
        CornerRadius::ZERO,
        Stroke::new(1.0, BEVEL_DARK),
        StrokeKind::Inside,
    );
    let midpoint = ((0.0 - min) / (max - min)).clamp(0.0, 1.0);
    let zero_y = track.bottom() - track.height() * midpoint;
    ui.painter().line_segment(
        [
            pos2(track.left() - 3.0, zero_y),
            pos2(track.right() + 3.0, zero_y),
        ],
        Stroke::new(1.0, Color32::from_rgb(70, 70, 70)),
    );
    let t = if max > min {
        ((*value - min) / (max - min)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let value_y = track.bottom() - track.height() * t;
    let fill_rect = Rect::from_two_pos(pos2(track.left(), zero_y), pos2(track.right(), value_y));
    if fill_rect.height() > 0.0 {
        ui.painter().rect_filled(
            fill_rect,
            CornerRadius::ZERO,
            if enabled {
                LCD_DIM
            } else {
                Color32::from_rgb(30, 50, 30)
            },
        );
    }
    let thumb = Rect::from_center_size(pos2(rect.center().x, value_y), vec2(16.0, 7.0));
    bevel_rect(
        ui.painter(),
        thumb,
        BTN_FACE,
        response.is_pointer_button_down_on(),
    );
    response
}
