use crate::core::fs::{MY_PC_PATH, MY_RECYCLE_BIN_PATH};
use crate::core::portable;
use crate::gui::i18n::I18n;
use crate::gui::icons::IconCache;
use crate::gui::theme::ThemePalette;
use crate::gui::utils::{clickable_icon, expand_environment_variables};
use crate::gui::windows::containers::enums::ItemViewerNavAction;
use crate::gui::windows::containers::structs::{
    Breadcrumb, ItemViewerDisplayMode, ItemViewerNavBarAction, RenderedBreadcrumb, TabView,
};
use eframe::egui;
use egui::text::{CCursor, CCursorRange};
use egui::{FontFamily, FontId};
use egui_phosphor::{fill, regular};
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn draw_itemviewer_navigation_bar(
    ui: &mut egui::Ui,
    i18n: &I18n,
    icon_cache: &IconCache,
    tab: &mut TabView,
    tab_id: u64,
    palette: &ThemePalette,
    is_favorited: bool,
    drag_active: bool,
    drag_hover_target: Option<PathBuf>,
) -> ItemViewerNavBarAction {
    let mut action = ItemViewerNavBarAction::default();
    let tabbar_rect = ui.available_rect_before_wrap();
    ui.set_clip_rect(tabbar_rect);

    let pointer_pos = ui.input(|i| i.pointer.interact_pos().or_else(|| i.pointer.hover_pos()));
    let pointer_released =
        ui.input(|i| i.pointer.any_released() && i.pointer.interact_pos().is_some());
    let hovered_target_ref = drag_hover_target.as_ref();
    let mut breadcrumb_drop_target: Option<PathBuf> = None;
    let pointer_in_tabbar = pointer_pos
        .map(|pos| tabbar_rect.contains(pos))
        .unwrap_or(false);
    let can_go_back = tab.nav.can_go_back();
    let can_go_forward = tab.nav.can_go_forward();
    let is_recycle_bin = tab.nav.is_recycle_bin();

    ui.horizontal(|ui| {
        ui.add_space(1.5);
        let toolbar_action = draw_navigation_bar_buttons(
            ui,
            i18n,
            palette,
            is_favorited,
            &tab.nav.current,
            tab.nav.is_root(),
            is_recycle_bin,
            can_go_back,
            can_go_forward,
            tab.display_mode,
        );

        merge_toolbar_action(&mut action, toolbar_action);
        if action.toggle_gallery {
            tab.display_mode = match tab.display_mode {
                ItemViewerDisplayMode::Details => ItemViewerDisplayMode::Gallery,
                ItemViewerDisplayMode::Gallery => ItemViewerDisplayMode::Details,
            };
        }

        ui.separator();

        let breadcrumb_width = ui.available_width();

        if tab.breadcrumb_path_editing {
            let text_edit_id = ui.id().with(("breadcrumbs_path_edit", tab_id));

            if tab.breadcrumb_select_all_on_focus {
                let mut state =
                    egui::widgets::text_edit::TextEditState::load(ui.ctx(), text_edit_id)
                        .unwrap_or_default();
                let cursor_end = CCursor::new(tab.breadcrumb_path_buffer.chars().count());
                state
                    .cursor
                    .set_char_range(Some(CCursorRange::two(CCursor::new(0), cursor_end)));
                state.store(ui.ctx(), text_edit_id);
            }

            let mut offset_x = 0.0;
            if tab.breadcrumb_path_error {
                let t = (ui.input(|i| i.time) - tab.breadcrumb_path_error_animation_time) as f32;

                if t < 0.4 {
                    let frequency = 30.0_f32;
                    let amplitude = 4.0 * (1.0 - t / 0.4); // decay
                    offset_x = (t * frequency).sin() * amplitude;
                }
            }

            ui.add_space(offset_x);

            let time_since_error = ui.input(|i| i.time) - tab.breadcrumb_path_error_animation_time;
            let error_strength = (1.0 - time_since_error * 2.0).clamp(0.0, 1.0);

            let stroke_color = if tab.breadcrumb_path_error {
                egui::Color32::from_rgba_premultiplied(255, 80, 80, (255.0 * error_strength) as u8)
            } else {
                ui.visuals().widgets.inactive.bg_stroke.color
            };

            let frame = egui::Frame::NONE
                .stroke(egui::Stroke::new(1.5, stroke_color))
                .corner_radius(egui::CornerRadius::same(4));

            let mut resp = frame
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut tab.breadcrumb_path_buffer)
                            .id(text_edit_id)
                            .frame(if tab.breadcrumb_path_error {
                                egui::Frame::NONE
                            } else {
                                egui::Frame::default()
                            })
                            .desired_width(ui.available_width() - 40.0)
                            .font(FontId::new(
                                palette.text_size,
                                egui::FontFamily::Proportional,
                            )),
                    )
                })
                .inner;

            if resp.changed() {
                tab.breadcrumb_path_error = false;
            }

            if tab.breadcrumb_path_error && resp.hovered() {
                resp = resp.on_hover_text(
                    egui::RichText::new(i18n.tr("tooltip_path_does_not_exist"))
                        .size(palette.tooltip_text_size)
                        .color(palette.tooltip_text_color),
                );
            }

            if !tab.breadcrumb_just_started_editing || tab.breadcrumb_path_error {
                resp.request_focus();
                tab.breadcrumb_just_started_editing = true;
            }

            if resp.has_focus() {
                tab.breadcrumb_select_all_on_focus = false;
            }

            action.is_breadcrumb_path_edit_active = resp.has_focus();

            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));

            let mut exit_edit_mode = false;

            if enter {
                let input = tab.breadcrumb_path_buffer.trim().trim_matches('"');
                let expanded_input = expand_environment_variables(input);
                let new_path = PathBuf::from(&expanded_input);

                if new_path.exists() {
                    action.nav_to = Some(new_path);
                    exit_edit_mode = true;
                } else {
                    println!(
                        "{}: {} ({}: {})",
                        i18n.tr("tooltip_invalid_path"),
                        tab.breadcrumb_path_buffer,
                        i18n.tr("tooltip_invalid_path_expanded"),
                        expanded_input
                    );
                    tab.breadcrumb_path_error = true;
                    tab.breadcrumb_path_error_animation_time = ui.input(|i| i.time);
                }
            } else if escape || resp.lost_focus() {
                tab.breadcrumb_path_buffer = tab.nav.current.to_string_lossy().to_string();
                exit_edit_mode = true;
            }

            if exit_edit_mode {
                tab.breadcrumb_path_editing = false;
                tab.breadcrumb_just_started_editing = false;
                tab.breadcrumb_path_error = false;
                tab.breadcrumb_path_error_animation_time = 0.0;

                ui.memory_mut(|mem| mem.surrender_focus(text_edit_id));
            }
        } else if tab.nav.is_root() || tab.nav.is_recycle_bin() {
            let pc_icon_path = PathBuf::from("C:\\");
            if tab.nav.is_recycle_bin() {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(regular::TRASH)
                            .size(14.0)
                            .color(palette.text_header_section),
                    )
                    .selectable(false),
                );
            } else if let Some(icon) = icon_cache.get(&pc_icon_path, true) {
                ui.add(egui::Image::new(&icon).fit_to_exact_size(egui::vec2(14.0, 14.0)));
            }

            if ui
                .add(
                    egui::Label::new(
                        egui::RichText::new(if tab.nav.is_recycle_bin() {
                            i18n.tr("recycle_bin")
                        } else {
                            i18n.tr("thispc")
                        })
                        .size(palette.text_size)
                        .color(palette.text_header_section),
                    )
                    .selectable(false)
                    .sense(egui::Sense::click()),
                )
                .clicked()
            {
                action.nav_to = Some(if tab.nav.is_recycle_bin() {
                    PathBuf::from(MY_RECYCLE_BIN_PATH)
                } else {
                    PathBuf::from(MY_PC_PATH)
                });
            }
        } else {
            let font_id = egui::FontId::new(palette.text_size, egui::FontFamily::Proportional);
            let breadcrumbs = build_breadcrumbs(&tab.nav.current);
            let segments = layout_breadcrumbs(ui, &breadcrumbs, breadcrumb_width, &font_id);
            let mut first = true;
            let mut breadcrumbs_right = 0.0;

            for crumb in &segments {
                if !first {
                    let old_spacing = ui.spacing().item_spacing;
                    ui.spacing_mut().item_spacing.x = 10.0;

                    ui.label(
                        egui::RichText::new(">")
                            .size(palette.text_size)
                            .color(palette.text_header_section),
                    );

                    ui.spacing_mut().item_spacing = old_spacing;
                }
                first = false;

                let resp = draw_breadcrumb(ui, crumb, palette);

                handle_breadcrumb_drag(
                    ui,
                    &resp,
                    crumb,
                    &segments,
                    palette,
                    drag_active,
                    hovered_target_ref,
                    pointer_pos,
                    pointer_released,
                    &mut breadcrumb_drop_target,
                    &mut action,
                );

                handle_breadcrumb_hover(ui, &resp);

                handle_breadcrumb_click(&resp, crumb, &segments, drag_active, &mut action);

                breadcrumbs_right = resp.rect.right();
            }

            // ---- Detect click in empty area to enter path editing ----
            let available_rect = ui.available_rect_before_wrap();
            let empty_area = egui::Rect::from_min_max(
                egui::pos2(breadcrumbs_right, available_rect.top()),
                available_rect.right_bottom(),
            );

            if ui
                .interact(
                    empty_area,
                    ui.id().with("breadcrumb_empty"),
                    egui::Sense::click(),
                )
                .clicked()
            {
                tab.breadcrumb_path_editing = true;
                tab.breadcrumb_path_buffer = tab.nav.current.to_string_lossy().to_string();
            }
        }
    });

    if action.nav.is_none() && pointer_in_tabbar && !tab.breadcrumb_path_editing {
        if can_go_back && ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Extra1)) {
            action.nav = Some(ItemViewerNavAction::Back);
        } else if can_go_forward
            && ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Extra2))
        {
            action.nav = Some(ItemViewerNavAction::Forward);
        }
    }

    action.move_files_to_breadcrumb_dir = breadcrumb_drop_target;
    action
}

fn nav_icon_button(
    ui: &mut egui::Ui,
    icon: &str,
    palette: &ThemePalette,
    enabled: bool,
    hover_text: &str,
) -> egui::Response {
    let font_id = egui::FontId::default();
    let resp = ui.add_enabled(
        enabled,
        egui::Label::new(
            egui::RichText::new(icon)
                .font(font_id.clone())
                .color(ui.visuals().text_color()),
        )
        .selectable(false)
        .sense(egui::Sense::click()),
    );

    if enabled && resp.hovered() {
        ui.painter().text(
            resp.rect.center(),
            egui::Align2::CENTER_CENTER,
            icon,
            font_id.clone(),
            palette.primary,
        );
    }

    let resp = resp.on_hover_text(
        egui::RichText::new(hover_text)
            .size(palette.tooltip_text_size)
            .color(palette.tooltip_text_color),
    );

    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

fn draw_navigation_bar_buttons(
    ui: &mut egui::Ui,
    i18n: &I18n,
    palette: &ThemePalette,
    is_favorited: bool,
    current_dir: &Path,
    is_root: bool,
    is_recycle_bin: bool,
    can_go_back: bool,
    can_go_forward: bool,
    display_mode: ItemViewerDisplayMode,
) -> ItemViewerNavBarAction {
    let mut action = ItemViewerNavBarAction::default();

    // Navigation buttons
    if nav_icon_button(
        ui,
        regular::ARROW_LEFT,
        palette,
        can_go_back,
        &i18n.tr("tooltip_nav_back"),
    )
    .clicked()
        && can_go_back
    {
        action.nav = Some(ItemViewerNavAction::Back);
    }

    if nav_icon_button(
        ui,
        regular::ARROW_RIGHT,
        palette,
        can_go_forward,
        &i18n.tr("tooltip_nav_forward"),
    )
    .clicked()
        && can_go_forward
    {
        action.nav = Some(ItemViewerNavAction::Forward);
    }

    let can_go_up = !is_root && !is_recycle_bin;
    if nav_icon_button(
        ui,
        regular::ARROW_UP,
        palette,
        can_go_up,
        &i18n.tr("tooltip_nav_up"),
    )
    .clicked()
        && can_go_up
    {
        action.nav = Some(ItemViewerNavAction::Up);
    }

    if clickable_icon(ui, regular::ARROWS_CLOCKWISE, palette)
        .on_hover_text(
            egui::RichText::new(i18n.tr("tooltip_refresh"))
                .size(palette.tooltip_text_size)
                .color(palette.tooltip_text_color),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        action.refresh_current_directory = true;
    }

    // Action buttons
    if nav_icon_button(
        ui,
        regular::FOLDER_PLUS,
        palette,
        !is_recycle_bin,
        &i18n.tr("tooltip_newfolder"),
    )
    .clicked()
        && !is_recycle_bin
    {
        action.create_folder = true;
    }

    if nav_icon_button(
        ui,
        regular::FILE_PLUS,
        palette,
        !is_recycle_bin,
        &i18n.tr("tooltip_newfile"),
    )
    .clicked()
        && !is_recycle_bin
    {
        action.create_file = true;
    }

    let star_icon = if is_favorited {
        fill::STAR
    } else {
        regular::STAR
    };
    let star_color = if is_favorited {
        palette.button_favorite_fill
    } else {
        palette.tooltip_text_color
    };

    let star_font = if is_favorited {
        FontId::new(palette.text_size, FontFamily::Name("phosphor_fill".into()))
    } else {
        FontId::new(palette.text_size, FontFamily::Proportional)
    };

    let star_resp = ui.add_enabled(
        !is_root && !is_recycle_bin,
        egui::Label::new(
            egui::RichText::new(star_icon)
                .font(star_font)
                .color(star_color),
        )
        .selectable(false)
        .sense(egui::Sense::click()),
    );

    let star_resp = if is_root || is_recycle_bin {
        star_resp.on_hover_text(
            egui::RichText::new(i18n.tr("tooltip_favorites_disabled"))
                .size(palette.tooltip_text_size)
                .color(palette.tooltip_text_color),
        )
    } else {
        star_resp
            .on_hover_text(
                egui::RichText::new(if is_favorited {
                    i18n.tr("tooltip_favorites_remove")
                } else {
                    i18n.tr("tooltip_favorites_add")
                })
                .size(palette.tooltip_text_size)
                .color(palette.tooltip_text_color),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
    };

    if star_resp.clicked() {
        if is_favorited {
            action.remove_favorite = true;
        } else {
            action.add_favorite = true;
        }
    }

    let gallery_color = if display_mode == ItemViewerDisplayMode::Gallery {
        palette.primary
    } else {
        ui.visuals().text_color()
    };
    if ui
        .add_enabled(
            !is_recycle_bin && !is_root,
            egui::Label::new(egui::RichText::new(regular::IMAGES_SQUARE).color(gallery_color))
                .selectable(false)
                .sense(egui::Sense::click()),
        )
        .on_hover_text(
            egui::RichText::new(i18n.tr("tooltip_gallery_view"))
                .size(palette.tooltip_text_size)
                .color(palette.tooltip_text_color),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
        && !is_recycle_bin
    {
        action.toggle_gallery = true;
    }

    if nav_icon_button(
        ui,
        regular::TERMINAL,
        palette,
        !is_recycle_bin,
        &i18n.tr("tooltip_open_terminal"),
    )
    .clicked()
        && !is_recycle_bin
    {
        open_default_terminal(current_dir);
    }

    action
}

pub(crate) fn open_default_terminal(current_dir: &Path) {
    let start_dir = if current_dir.to_string_lossy() == MY_PC_PATH || !current_dir.exists() {
        dirs::home_dir().unwrap_or_else(|| current_dir.to_path_buf())
    } else {
        current_dir.to_path_buf()
    };

    let launched = Command::new("wt.exe")
        .arg("-d")
        .arg(&start_dir)
        .spawn()
        .is_ok()
        || Command::new("powershell.exe")
            .current_dir(&start_dir)
            .spawn()
            .is_ok()
        || Command::new("cmd.exe")
            .current_dir(&start_dir)
            .spawn()
            .is_ok();

    if !launched {
        eprintln!("Failed to open terminal");
    }
}

fn measure_breadcrumb_width(ui: &egui::Ui, font_id: &egui::FontId, text: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_owned(), font_id.clone(), ui.visuals().text_color())
        .size()
        .x
}

fn truncate_breadcrumb_to_width(
    ui: &egui::Ui,
    font_id: &egui::FontId,
    text: &str,
    max_width: f32,
) -> (String, bool) {
    if measure_breadcrumb_width(ui, font_id, text) <= max_width {
        return (text.to_owned(), false);
    }

    let ellipsis = "...";
    let ellipsis_width = measure_breadcrumb_width(ui, font_id, ellipsis);

    let mut result = String::new();

    for ch in text.chars() {
        let candidate = format!("{result}{ch}");

        if measure_breadcrumb_width(ui, font_id, &candidate) + ellipsis_width > max_width {
            break;
        }

        result.push(ch);
    }

    result.push_str(ellipsis);

    (result, true)
}

fn build_breadcrumbs(path: &Path) -> Vec<Breadcrumb> {
    use std::path::Component;

    if portable::is_portable_path(&path.to_path_buf()) {
        return portable::build_breadcrumb_segments(&path.to_path_buf())
            .unwrap_or_default()
            .into_iter()
            .map(|(label, path)| Breadcrumb { label, path })
            .collect();
    }

    let mut breadcrumbs = Vec::new();
    let mut current = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                current.push(prefix.as_os_str());

                breadcrumbs.push(Breadcrumb {
                    label: prefix.as_os_str().to_string_lossy().into_owned(),
                    path: current.clone(),
                });
            }

            Component::RootDir => {
                current.push(Path::new("\\"));
            }

            Component::Normal(name) => {
                current.push(name);

                breadcrumbs.push(Breadcrumb {
                    label: name.to_string_lossy().into_owned(),
                    path: current.clone(),
                });
            }

            _ => {}
        }
    }

    breadcrumbs
}

fn layout_breadcrumbs(
    ui: &egui::Ui,
    breadcrumbs: &[Breadcrumb],
    available_width: f32,
    font_id: &egui::FontId,
) -> Vec<RenderedBreadcrumb> {
    const SEPARATOR_WIDTH: f32 = 18.0;
    const ITEM_PADDING: f32 = 12.0;

    if breadcrumbs.is_empty() {
        return Vec::new();
    }

    // Measure every breadcrumb once.
    let measured: Vec<RenderedBreadcrumb> = breadcrumbs
        .iter()
        .map(|crumb| RenderedBreadcrumb {
            label: crumb.label.clone(),
            full_label: crumb.label.clone(),
            path: crumb.path.clone(),
            truncated: false,
            is_ellipsis: false,
            width: measure_breadcrumb_width(ui, font_id, &crumb.label) + ITEM_PADDING,
        })
        .collect();

    // Fast path: everything fits.
    let total_width: f32 = measured.iter().map(|c| c.width).sum::<f32>()
        + SEPARATOR_WIDTH * measured.len().saturating_sub(1) as f32;

    if total_width <= available_width {
        return measured;
    }

    let ellipsis_width = measure_breadcrumb_width(ui, font_id, "...") + ITEM_PADDING;

    let mut result = Vec::new();

    // Always keep the first breadcrumb.
    result.push(measured[0].clone());

    let mut used_width = measured[0].width;

    // Keep as many breadcrumbs from the right as possible.
    let mut right_side = Vec::new();

    for crumb in measured.iter().skip(1).rev() {
        let needed = crumb.width + SEPARATOR_WIDTH;

        // Leave room for:
        //   - separator before "..."
        //   - "..."
        //   - separator after "..."
        let reserved = SEPARATOR_WIDTH + ellipsis_width + SEPARATOR_WIDTH;

        if used_width + reserved + needed <= available_width {
            used_width += needed;
            right_side.push(crumb.clone());
        } else {
            break;
        }
    }

    right_side.reverse();

    let omitted = right_side.len() < measured.len() - 1;

    if omitted {
        result.push(RenderedBreadcrumb {
            label: "...".into(),
            full_label: "...".into(),
            path: measured[0].path.clone(),
            truncated: false,
            is_ellipsis: true,
            width: ellipsis_width,
        });
    }

    result.extend(right_side);

    // Truncate only the final breadcrumb if needed.
    if let Some(last_index) = result.len().checked_sub(1) {
        let consumed: f32 = result[..last_index]
            .iter()
            .map(|c| c.width + SEPARATOR_WIDTH)
            .sum();

        let remaining = (available_width - consumed).max(80.0);

        let full = result[last_index].full_label.clone();

        let (label, truncated) = truncate_breadcrumb_to_width(ui, font_id, &full, remaining);

        result[last_index].label = label;
        result[last_index].truncated = truncated;
    }

    result
}

fn draw_breadcrumb(
    ui: &mut egui::Ui,
    crumb: &RenderedBreadcrumb,
    palette: &ThemePalette,
) -> egui::Response {
    let inner = egui::Frame::NONE
        .fill(egui::Color32::TRANSPARENT)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .corner_radius(egui::CornerRadius::same(palette.medium_radius))
        .show(ui, |ui| {
            let resp = ui.add(
                egui::Label::new(
                    egui::RichText::new(&crumb.label)
                        .size(palette.text_size)
                        .color(palette.text_header_section),
                )
                .selectable(false)
                .sense(egui::Sense::click()),
            );

            if crumb.truncated {
                resp.on_hover_text(&crumb.full_label)
            } else {
                resp
            }
        });

    inner.response.union(inner.inner)
}

fn handle_breadcrumb_hover(ui: &egui::Ui, resp: &egui::Response) {
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}

fn handle_breadcrumb_click(
    resp: &egui::Response,
    crumb: &RenderedBreadcrumb,
    segments: &[RenderedBreadcrumb],
    drag_active: bool,
    action: &mut ItemViewerNavBarAction,
) {
    if drag_active || !resp.clicked() {
        return;
    }

    if crumb.is_ellipsis {
        if let Some(first) = segments.first() {
            action.nav_to = Some(first.path.clone());
        }
    } else {
        action.nav_to = Some(crumb.path.clone());
    }
}

fn handle_breadcrumb_drag(
    ui: &egui::Ui,
    resp: &egui::Response,
    crumb: &RenderedBreadcrumb,
    segments: &[RenderedBreadcrumb],
    palette: &ThemePalette,
    drag_active: bool,
    hovered_target_ref: Option<&PathBuf>,
    pointer_pos: Option<egui::Pos2>,
    pointer_released: bool,
    breadcrumb_drop_target: &mut Option<PathBuf>,
    action: &mut ItemViewerNavBarAction,
) {
    if !drag_active {
        return;
    }

    let breadcrumb_target_path = if crumb.is_ellipsis {
        segments.first().map(|c| &c.path)
    } else {
        Some(&crumb.path)
    };

    let hovered = breadcrumb_target_path
        .and_then(|target_path| hovered_target_ref.map(|target| target == target_path))
        .unwrap_or_else(|| {
            pointer_pos
                .map(|pointer| resp.rect.contains(pointer))
                .unwrap_or(false)
        });

    if !hovered {
        return;
    }

    let painter = ui
        .ctx()
        .layer_painter(egui::LayerId::new(
            egui::Order::Background,
            ui.id().with("breadcrumb_drop_bg").with(&crumb.path),
        ))
        .with_clip_rect(ui.clip_rect());

    painter.rect_filled(
        resp.rect,
        egui::CornerRadius::same(palette.medium_radius),
        palette.primary_hover,
    );

    if pointer_released {
        let target = if crumb.is_ellipsis {
            segments.first().map(|c| c.path.clone())
        } else {
            Some(crumb.path.clone())
        };

        if let Some(target) = target {
            *breadcrumb_drop_target = Some(target);
            action.move_files_to_breadcrumb_dir_rect = Some(resp.rect);
        }
    }
}

fn merge_toolbar_action(action: &mut ItemViewerNavBarAction, toolbar: ItemViewerNavBarAction) {
    if action.nav.is_none() {
        action.nav = toolbar.nav;
    }

    if action.nav_to.is_none() {
        action.nav_to = toolbar.nav_to;
    }

    action.refresh_current_directory |= toolbar.refresh_current_directory;
    action.create_folder |= toolbar.create_folder;
    action.create_file |= toolbar.create_file;
    action.add_favorite |= toolbar.add_favorite;
    action.remove_favorite |= toolbar.remove_favorite;
    action.toggle_gallery |= toolbar.toggle_gallery;
}
