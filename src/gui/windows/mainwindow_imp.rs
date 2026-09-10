use crate::core::drives::{get_drive_infos, is_raw_physical_drive_path};
use crate::core::fs::{FileItem, get_shell_item_metadata};
use crate::core::fs::{MY_RECYCLE_BIN_PATH, parallel_directory_scan, scan_dir_async};
use crate::core::indexer::{
    DirectorySettingsSnapshot, load_app_settings, save_app_settings, save_favorites, save_tags,
    save_theme_settings,
};
use crate::gui::MainWindow;
use crate::gui::i18n::I18n;
use crate::gui::theme::{
    ThemeMode, ThemePalette, apply_font_to_context, get_default_palette, set_palette,
};
use crate::gui::utils::{
    ClipboardFileRead, SortColumn, SortKey, clear_clipboard_files, is_clipboard_cut,
    read_clipboard_files, set_clipboard_files, shell_delete_to_recycle_bin, show_copy_move_dialog,
    sort_files_by_keys,
};
use crate::gui::windows::about::draw_about_window;
use crate::gui::windows::containers::enums::{
    ItemViewerAction, ItemViewerContextAction, ItemViewerHeaderColumn, ItemViewerNavAction,
};
use crate::gui::windows::containers::itemviewer_navbar::open_default_terminal;
use crate::gui::windows::containers::structs::{
    FavoriteItem, FilterState, GalleryThumbnailSize, ItemViewerColumnFitRequest,
    ItemViewerColumnState, ItemViewerDisplayMode, ItemViewerFolderSizeState,
    ItemViewerNavBarAction, RenameState, SidebarAction, SplitSide, TabState, TabView, TabsAction,
    TopbarAction,
};
use crate::gui::windows::customizetheme::draw_theme_customizer;
use crate::gui::windows::enums::{SettingsAction, ThemeCustomizerAction};
use crate::gui::windows::settings::draw_settings_window;
use crate::gui::windows::structs::{AppSettings, Navigation, ThemeCustomizer};
use crate::gui::windows::windowsoverrides::mark_clipboard_dirty;
use crate::gui::windows::windowsoverrides::toggle_window_fullscreen;
use crossbeam_channel::Receiver;
use crossbeam_channel::{Sender, unbounded};
use eframe::egui;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    IShellItem, SEE_MASK_INVOKEIDLIST, SHELLEXECUTEINFOW, ShellExecuteExW, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Error, HRESULT};

use windows::{Win32::System::Com::*, Win32::UI::Shell::*, core::*};

pub(crate) fn default_column_state(settings: &AppSettings) -> ItemViewerColumnState {
    ItemViewerColumnState::from_orders(
        settings.item_viewer_file_column_order.clone(),
        settings.item_viewer_drive_column_order.clone(),
        settings.recycle_bin_column_order.clone(),
        settings.item_viewer_file_column_sizes.clone(),
        settings.item_viewer_drive_column_sizes.clone(),
        settings.recycle_bin_column_sizes.clone(),
    )
}

pub(crate) fn default_directory_settings_snapshot(directory: PathBuf) -> DirectorySettingsSnapshot {
    let default_settings = AppSettings::default();

    DirectorySettingsSnapshot {
        directory,
        item_viewer_file_column_order: default_settings.item_viewer_file_column_order,
        item_viewer_drive_column_order: default_settings.item_viewer_drive_column_order,
        recycle_bin_column_order: default_settings.recycle_bin_column_order,
        item_viewer_file_column_sizes: default_settings.item_viewer_file_column_sizes,
        item_viewer_drive_column_sizes: default_settings.item_viewer_drive_column_sizes,
        recycle_bin_column_sizes: default_settings.recycle_bin_column_sizes,
        filter_query: String::new(),
        display_mode: ItemViewerDisplayMode::Details,
        gallery_thumbnail_size: GalleryThumbnailSize::Medium,
        sort_column: SortColumn::Name,
        sort_ascending: true,
        sort_keys: vec![SortKey {
            column: SortColumn::Name,
            ascending: true,
        }],
    }
}

pub(crate) fn directory_settings_snapshot_for_view(view: &TabView) -> DirectorySettingsSnapshot {
    DirectorySettingsSnapshot {
        directory: view.nav.current.clone(),
        item_viewer_file_column_order: view.column_state.file_column_order.clone(),
        item_viewer_drive_column_order: view.column_state.drive_column_order.clone(),
        recycle_bin_column_order: view.column_state.recycle_bin_column_order.clone(),
        item_viewer_file_column_sizes: view.column_state.file_column_sizes.clone(),
        item_viewer_drive_column_sizes: view.column_state.drive_column_sizes.clone(),
        recycle_bin_column_sizes: view.column_state.recycle_bin_column_sizes.clone(),
        filter_query: view.item_viewer_filter_state.query.clone(),
        display_mode: view.display_mode,
        gallery_thumbnail_size: view.gallery_state.thumbnail_size,
        sort_column: view.sort_column,
        sort_ascending: view.sort_ascending,
        sort_keys: view.sort_keys.clone(),
    }
}

pub(crate) fn apply_directory_settings_to_view(view: &mut TabView, settings: &AppSettings) {
    let snapshot = settings
        .directory_settings
        .iter()
        .find(|entry| entry.directory == view.nav.current);

    if let Some(snapshot) = snapshot {
        view.column_state = ItemViewerColumnState::from_orders(
            snapshot.item_viewer_file_column_order.clone(),
            snapshot.item_viewer_drive_column_order.clone(),
            snapshot.recycle_bin_column_order.clone(),
            snapshot.item_viewer_file_column_sizes.clone(),
            snapshot.item_viewer_drive_column_sizes.clone(),
            snapshot.recycle_bin_column_sizes.clone(),
        );
        view.item_viewer_filter_state = FilterState {
            active: !snapshot.filter_query.is_empty(),
            query: snapshot.filter_query.clone(),
            ..Default::default()
        };
        view.display_mode = snapshot.display_mode;
        view.gallery_state
            .set_thumbnail_size(snapshot.gallery_thumbnail_size);
        view.sort_keys = if snapshot.sort_keys.is_empty() {
            vec![SortKey {
                column: snapshot.sort_column,
                ascending: snapshot.sort_ascending,
            }]
        } else {
            snapshot.sort_keys.clone()
        };
        let primary = view.sort_keys[0];
        view.sort_column = primary.column;
        view.sort_ascending = primary.ascending;
    } else {
        view.column_state = default_column_state(settings);
        view.item_viewer_filter_state = FilterState::default();
        view.display_mode = ItemViewerDisplayMode::Details;
        view.gallery_state = Default::default();
        view.sort_keys = vec![SortKey {
            column: settings.sort_column,
            ascending: settings.sort_ascending,
        }];
        view.sort_column = settings.sort_column;
        view.sort_ascending = settings.sort_ascending;
    }
}

pub(crate) fn persist_directory_settings_snapshot(
    entries: &mut Vec<DirectorySettingsSnapshot>,
    snapshot: DirectorySettingsSnapshot,
) -> bool {
    let default_snapshot = default_directory_settings_snapshot(snapshot.directory.clone());

    if snapshot == default_snapshot {
        if let Some(pos) = entries
            .iter()
            .position(|entry| entry.directory == snapshot.directory)
        {
            entries.remove(pos);
            return true;
        }
        return false;
    }

    if let Some(pos) = entries
        .iter()
        .position(|entry| entry.directory == snapshot.directory)
    {
        if entries[pos] == snapshot {
            return false;
        }
        entries[pos] = snapshot;
        true
    } else {
        entries.push(snapshot);
        true
    }
}

impl Drop for MainWindow {
    fn drop(&mut self) {
        self.cleanup_resources();
    }
}

impl MainWindow {
    /// Comprehensive validation for submitted filenames
    /// Used when user submits/commits the filename (Enter, create new file/folder)
    fn is_submitted_filename_valid(name: &str) -> bool {
        if name.is_empty() {
            return false;
        }

        if name.len() > 255 {
            return false;
        }

        // Cannot be "." or ".."
        if name == "." || name == ".." {
            return false;
        }

        // Cannot start or end with space
        if name.starts_with(' ') || name.ends_with(' ') {
            return false;
        }

        // Cannot end with dot
        if name.ends_with('.') {
            return false;
        }

        // Invalid characters
        let invalid_chars = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
        if name.chars().any(|c| invalid_chars.contains(&c)) {
            return false;
        }

        // Control characters (0x00–0x1F)
        if name.chars().any(|c| c < '\u{20}') {
            return false;
        }

        // Reserved names (check base name before extension)
        let reserved_names = [
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ];

        let base = name.split('.').next().unwrap_or("");
        let base_upper = base.to_uppercase();

        if reserved_names.contains(&base_upper.as_str()) {
            return false;
        }

        // Optional: max length
        if name.len() > 255 {
            return false;
        }

        true
    }

    pub fn current_nav(&self) -> &Navigation {
        &self.active_tab().view(self.focused_split).nav
    }

    pub fn current_nav_mut(&mut self) -> &mut Navigation {
        let side = self.focused_split;
        &mut self.active_tab_mut().view_mut(side).nav
    }

    pub fn open_new_tab(&mut self, path: PathBuf) {
        let nav = Navigation::new(path);
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let (sort_column, sort_ascending) = {
            let view = self.active_tab().view(self.focused_split);
            (view.sort_column, view.sort_ascending)
        };
        self.tabs
            .push(TabState::new(id, nav, sort_column, sort_ascending));
        let current_settings = self.settings_window.current_settings.clone();
        apply_directory_settings_to_view(
            &mut self.tabs.last_mut().unwrap().primary_view,
            &current_settings,
        );
        self.active_tab = self.tabs.len() - 1;
        self.mark_tab_infos_dirty();
    }

    pub fn open_startup_paths(&mut self, paths: &[PathBuf]) {
        let Some(first_path) = paths.first() else {
            return;
        };

        let pinned_count = self.settings_window.current_settings.pinned_tabs.len();
        if pinned_count == 0 {
            self.tabs[0].primary_view.nav = Navigation::new(first_path.clone());
            self.active_tab = 0;
            self.focused_split = SplitSide::Primary;
            self.load_path();
        } else {
            self.open_new_tab(first_path.clone());
            self.load_path();
        }

        for path in paths.iter().skip(1) {
            self.open_new_tab(path.clone());
        }

        if pinned_count > 0 {
            self.active_tab = pinned_count;
            self.focused_split = SplitSide::Primary;
            self.pending_tab_scroll_id = self.tabs.get(self.active_tab).map(|tab| tab.id);
            self.load_path();
        } else {
            self.active_tab = 0;
            self.focused_split = SplitSide::Primary;
            self.pending_tab_scroll_id = self.tabs.first().map(|tab| tab.id);
        }
        self.mark_tab_infos_dirty();
    }

    pub fn default_favorites(&self) -> Vec<FavoriteItem> {
        let mut favorites = Vec::new();
        if let Some(home) = dirs::home_dir() {
            let desktop = home.join("Desktop");
            favorites.push(FavoriteItem {
                path: desktop,
                label: "Desktop".to_string(),
            });
            let documents = home.join("Documents");
            favorites.push(FavoriteItem {
                path: documents,
                label: "Documents".to_string(),
            });
            let downloads = home.join("Downloads");
            favorites.push(FavoriteItem {
                path: downloads,
                label: "Downloads".to_string(),
            });
            let pictures = home.join("Pictures");
            favorites.push(FavoriteItem {
                path: pictures,
                label: "Pictures".to_string(),
            });
        }
        favorites
    }

    pub fn toggle_sort(&mut self, col: SortColumn, additive: bool, remove: bool) {
        let side = self.focused_split;
        let sort_keys = {
            let view = self.active_tab_mut().view_mut(side);
            if remove {
                view.sort_keys.retain(|key| key.column != col);
                if view.sort_keys.is_empty() {
                    view.sort_keys.push(SortKey {
                        column: SortColumn::Name,
                        ascending: true,
                    });
                }
            } else if additive {
                if let Some(key) = view.sort_keys.iter_mut().find(|key| key.column == col) {
                    key.ascending = !key.ascending;
                } else {
                    view.sort_keys.push(SortKey {
                        column: col,
                        ascending: true,
                    });
                }
            } else {
                let ascending = view
                    .sort_keys
                    .first()
                    .filter(|key| key.column == col)
                    .map(|key| !key.ascending)
                    .unwrap_or(true);
                view.sort_keys = vec![SortKey {
                    column: col,
                    ascending,
                }];
            }
            let primary = view.sort_keys[0];
            view.sort_column = primary.column;
            view.sort_ascending = primary.ascending;
            view.sort_keys.clone()
        };

        sort_files_by_keys(&mut self.active_tab_mut().view_mut(side).files, &sort_keys);

        let snapshot = directory_settings_snapshot_for_view(self.active_tab().view(side));
        let _ = persist_directory_settings_snapshot(
            &mut self.settings_window.current_settings.directory_settings,
            snapshot,
        );

        self.save_app_settings_to_disk();
    }

    fn save_app_settings_to_disk(&self) {
        save_app_settings(
            self.settings_window
                .current_settings
                .folder_scanning_enabled,
            self.settings_window
                .current_settings
                .show_hidden_files_folders,
            self.settings_window.current_settings.show_item_viewer_icons,
            self.settings_window
                .current_settings
                .windows_context_menu_enabled,
            &self.settings_window.current_settings.window_size_mode,
            &self.settings_window.current_settings.start_path,
            Some(match self.theme {
                ThemeMode::Dark => "dark",
                ThemeMode::Light => "light",
            }),
            &self.settings_window.current_settings.pinned_tabs,
            self.settings_window.current_settings.time_format_24h,
            self.settings_window.current_settings.sort_column,
            self.settings_window.current_settings.sort_ascending,
            &self.settings_window.current_settings.language,
            self.settings_window.current_settings.date_style,
            &self
                .settings_window
                .current_settings
                .item_viewer_file_column_order,
            &self
                .settings_window
                .current_settings
                .item_viewer_drive_column_order,
            &self
                .settings_window
                .current_settings
                .recycle_bin_column_order,
            &self
                .settings_window
                .current_settings
                .item_viewer_file_column_sizes,
            &self
                .settings_window
                .current_settings
                .item_viewer_drive_column_sizes,
            &self
                .settings_window
                .current_settings
                .recycle_bin_column_sizes,
            &self.settings_window.current_settings.directory_settings,
        );
    }

    fn apply_item_viewer_column_order(
        &mut self,
        side: SplitSide,
        is_drive_view: bool,
        is_recycle_bin_view: bool,
        order: &[ItemViewerHeaderColumn],
    ) {
        let view = self.active_tab_mut().view_mut(side);
        let current_order = view
            .column_state
            .order_mut(is_drive_view, is_recycle_bin_view);
        current_order.clear();
        current_order.extend_from_slice(order);
        view.column_state.layout_generation = view.column_state.layout_generation.wrapping_add(1);
    }

    pub fn load_path(&mut self) {
        self.load_view(self.focused_split);
    }

    pub(crate) fn load_view(&mut self, side: SplitSide) {
        let (current_path, is_root) = {
            let view = self.active_tab().view(side);
            (view.nav.current.clone(), view.nav.is_root())
        };

        let current_settings = self.settings_window.current_settings.clone();

        {
            let view = self.active_tab_mut().view_mut(side);
            apply_directory_settings_to_view(view, &current_settings);
            view.files.clear();
            view.rx = None;
            view.size_req_tx = None;
            view.size_rx = None;
            view.pending_size_queue.clear();
            view.pending_size_set.clear();
            view.is_loading = false;
            view.explorer_state.selected_paths.clear();
            view.explorer_state.selection_anchor = None;
            view.explorer_state.selection_focus = None;
            view.item_viewer_filter_state.dirty = true;
            view.item_viewer_filter_state.cached_indices.clear();
        }
        self.folder_sizes.clear();
        self.file_size_text_cache.clear();
        self.folder_size_text_cache.clear();
        self.drive_size_text_cache.clear();

        if is_raw_physical_drive_path(&current_path) {
            self.active_tab_mut()
                .view_mut(side)
                .explorer_state
                .non_ntfs_popup_path = Some(current_path.clone());
            return;
        }

        if current_path.to_string_lossy() == MY_RECYCLE_BIN_PATH {
            self.load_recycle_bin_view(side);
            return;
        }

        if is_root {
            let view = self.active_tab_mut().view_mut(side);
            for d in get_drive_infos() {
                let label = d.display;
                let path = d.path;
                if let (Some(total), Some(free)) = (d.total_space, d.free_space) {
                    view.files.push(FileItem::with_drive_info(
                        label, path, true, false, None, None, None, None, None, None, None, None,
                        None, total, free,
                    ));
                } else {
                    view.files.push(FileItem::new(
                        label, path, true, false, None, None, None, None, None, None, None, None,
                        None,
                    ));
                }
            }

            sort_files_by_keys(&mut view.files, &view.sort_keys);
            return;
        }

        // Async directory listing
        let (tx, rx) = unbounded();
        scan_dir_async(
            current_path,
            tx,
            self.settings_window.current_settings.date_style,
            self.settings_window.current_settings.time_format_24h,
        );
        let folder_scanning_enabled = self
            .settings_window
            .current_settings
            .folder_scanning_enabled;
        let shutdown = Arc::clone(&self.shutdown);
        let view = self.active_tab_mut().view_mut(side);
        view.rx = Some(rx);
        view.is_loading = true;

        // Setup folder size calculation channels only if folder scanning is enabled
        if folder_scanning_enabled {
            let (size_req_tx, size_req_rx) = unbounded::<PathBuf>();
            let (size_done_tx, size_done_rx) = unbounded::<(PathBuf, u64, bool)>();
            view.size_req_tx = Some(size_req_tx);
            view.size_rx = Some(size_done_rx);

            // Spawn a thread pool to handle folder size requests in parallel
            let num_threads = num_cpus::get().max(2); // use all available cores
            view.size_threads =
                calculate_folder_sizes_parallel(size_req_rx, size_done_tx, shutdown, num_threads);
        }
    }

    pub fn load_recycle_bin_view(&mut self, side: SplitSide) {
        use windows::Win32::System::SystemServices::{SFGAO_FOLDER, SFGAO_HIDDEN};
        use windows::Win32::UI::Shell::{
            BHID_EnumItems, FOLDERID_RecycleBinFolder, IEnumShellItems, ILFree, ILGetSize,
            IShellItem, SHCreateItemFromIDList, SHGetIDListFromObject, SHGetKnownFolderIDList,
            SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_FILESYSPATH, SIGDN_NORMALDISPLAY,
        };

        let mut recycle_items = Vec::new();

        unsafe {
            let recycle_pidl = match SHGetKnownFolderIDList(&FOLDERID_RecycleBinFolder, 0, None) {
                Ok(pidl) => pidl,
                Err(err) => {
                    eprintln!("Failed to resolve Recycle Bin: {:?}", err);
                    return;
                }
            };

            let recycle_item: IShellItem = match SHCreateItemFromIDList(recycle_pidl) {
                Ok(item) => item,
                Err(err) => {
                    eprintln!("Failed to open Recycle Bin shell item: {:?}", err);
                    CoTaskMemFree(Some(recycle_pidl as _));
                    return;
                }
            };

            let enum_items: IEnumShellItems =
                match recycle_item.BindToHandler(None, &BHID_EnumItems) {
                    Ok(items) => items,
                    Err(err) => {
                        eprintln!("Failed to enumerate Recycle Bin items: {:?}", err);
                        CoTaskMemFree(Some(recycle_pidl as _));
                        return;
                    }
                };

            loop {
                let mut fetched_items: [Option<IShellItem>; 1] = [None];
                if let Err(err) = enum_items.Next(&mut fetched_items, None) {
                    eprintln!("Failed to read Recycle Bin item: {:?}", err);
                    break;
                }

                if fetched_items[0].is_none() {
                    break;
                }

                let Some(item) = fetched_items[0].take() else {
                    continue;
                };

                let recycle_bin_pidl = match SHGetIDListFromObject(&item) {
                    Ok(pidl) => {
                        let pidl_size = ILGetSize(Some(pidl as _)) as usize;
                        let mut pidl_bytes = vec![0u8; pidl_size];
                        std::ptr::copy_nonoverlapping(
                            pidl as *const u8,
                            pidl_bytes.as_mut_ptr(),
                            pidl_size,
                        );
                        ILFree(Some(pidl as _));
                        Some(pidl_bytes)
                    }
                    Err(err) => {
                        eprintln!("Failed to capture Recycle Bin PIDL: {:?}", err);
                        None
                    }
                };

                let display_name = item
                    .GetDisplayName(SIGDN_NORMALDISPLAY)
                    .ok()
                    .and_then(|name| {
                        let text = pwstr_to_string(name);
                        CoTaskMemFree(Some(name.0 as _));
                        if text.is_empty() { None } else { Some(text) }
                    })
                    .unwrap_or_else(|| "Unknown".to_string());

                let path = item
                    .GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING)
                    .or_else(|_| item.GetDisplayName(SIGDN_FILESYSPATH))
                    .ok()
                    .map(|name| {
                        let text = pwstr_to_string(name);
                        CoTaskMemFree(Some(name.0 as _));
                        text
                    });

                let Some(path) = path.map(PathBuf::from) else {
                    continue;
                };

                let attrs = item.GetAttributes(SFGAO_FOLDER | SFGAO_HIDDEN).ok();
                let is_dir = attrs.is_some_and(|flags| flags.contains(SFGAO_FOLDER));
                let is_hidden = attrs.is_some_and(|flags| flags.contains(SFGAO_HIDDEN));

                let (
                    file_size,
                    modified_time,
                    created_time,
                    deleted_time,
                    modified_time_raw,
                    created_time_raw,
                    deleted_time_raw,
                    original_object_name,
                    original_directory,
                ) = get_shell_item_metadata(
                    &item,
                    self.settings_window.current_settings.date_style,
                    self.settings_window.current_settings.time_format_24h,
                );

                recycle_items.push(FileItem::new(
                    original_object_name.unwrap_or(display_name),
                    path,
                    is_dir,
                    is_hidden,
                    recycle_bin_pidl,
                    file_size,
                    modified_time,
                    created_time,
                    deleted_time,
                    modified_time_raw,
                    created_time_raw,
                    deleted_time_raw,
                    original_directory,
                ));
            }

            CoTaskMemFree(Some(recycle_pidl as _));
        }

        let view = self.active_tab_mut().view_mut(side);
        view.files = recycle_items;
        sort_files_by_keys(&mut view.files, &view.sort_keys);
    }

    pub fn create_new_folder(&mut self) {
        if self.current_nav().is_root() || self.current_nav().is_recycle_bin() {
            return;
        }

        let base = self.current_nav().current.clone();
        let mut name = "New Folder".to_string();
        let mut counter = 1;
        let mut path = base.join(&name);

        // Find a valid, non-existent name
        while path.exists() {
            counter += 1;
            name = format!("New Folder ({})", counter);
            path = base.join(&name);
        }

        // Validate the final name before creating
        if Self::is_submitted_filename_valid(&name) {
            if std::fs::create_dir(&path).is_ok() {
                let path_for_selection = path.clone();
                self.load_path();
                // Immediately start renaming the new folder
                self.rename_state = Some(RenameState {
                    path: path.clone(),
                    new_name: name,
                    should_focus: true,
                    validation_error_show: false,
                });
                let side = self.focused_split;
                self.active_tab_mut()
                    .view_mut(side)
                    .explorer_state
                    .pending_selection_paths = Some(vec![path_for_selection]);
            }
        } else {
            // Don't create the folder, just start rename with invalid name so user can fix it
            self.rename_state = Some(RenameState {
                path: base.clone(), // Use parent directory as the path since we're not creating yet
                new_name: name,
                should_focus: true,
                validation_error_show: true, // Show error immediately
            });
        }
    }

    pub fn create_new_file(&mut self) {
        if self.current_nav().is_root() || self.current_nav().is_recycle_bin() {
            return;
        }

        let base = self.current_nav().current.clone();
        let mut name = "New File.txt".to_string();
        let mut counter = 1;
        let mut path = base.join(&name);

        // Find a valid, non-existent name
        while path.exists() {
            counter += 1;
            name = format!("New File ({}).txt", counter);
            path = base.join(&name);
        }

        // Validate the final name before creating
        if Self::is_submitted_filename_valid(&name) {
            // Create an empty file
            if std::fs::write(&path, "").is_ok() {
                let path_for_selection = path.clone();
                self.load_path();
                // Immediately start renaming the new file
                self.rename_state = Some(RenameState {
                    path: path.clone(),
                    new_name: name,
                    should_focus: true,
                    validation_error_show: false,
                });
                let side = self.focused_split;
                self.active_tab_mut()
                    .view_mut(side)
                    .explorer_state
                    .pending_selection_paths = Some(vec![path_for_selection]);
            }
        } else {
            // Don't create the file, just start rename with invalid name so user can fix it
            self.rename_state = Some(RenameState {
                path: base.clone(), // Use parent directory as the path since we're not creating yet
                new_name: name,
                should_focus: true,
                validation_error_show: true, // Show error immediately
            });
        }
    }

    pub fn add_favorite(&mut self) {
        if self.current_nav().is_root() || self.current_nav().is_recycle_bin() {
            return;
        }

        let path = self.current_nav().current.clone();
        if self
            .sidebar_state
            .favorites
            .iter()
            .any(|fav| fav.path == path)
        {
            return;
        }

        let label = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());

        self.sidebar_state
            .favorites
            .push(FavoriteItem { path, label });
        self.persist_favorites();
    }

    pub fn remove_favorite(&mut self, path: &PathBuf) {
        self.sidebar_state.favorites.retain(|fav| &fav.path != path);
        self.persist_favorites();
        if self
            .sidebar_state
            .item_clicked
            .as_ref()
            .map(|p| p == path)
            .unwrap_or(false)
        {
            self.sidebar_state.item_clicked = None;
        }
    }

    pub fn persist_favorites(&self) {
        let items: Vec<String> = self
            .sidebar_state
            .favorites
            .iter()
            .map(|fav| fav.path.display().to_string())
            .collect();
        save_favorites('C', &items);
    }

    pub fn persist_tags(&self) {
        save_tags(&self.tags_state.to_snapshot());
    }

    fn move_tagged_paths_to_dir(&mut self, sources: &[PathBuf], target_dir: &Path) -> bool {
        let mut changed = false;

        for source in sources {
            if let Some(file_name) = source.file_name() {
                let new_path = target_dir.join(file_name);
                changed |= self.tags_state.remap_path_prefix(source, &new_path);
            }
        }

        changed
    }

    pub fn handle_context_action(&mut self, action: ItemViewerContextAction) {
        match action {
            ItemViewerContextAction::Cut(paths) => {
                if set_clipboard_files(self.hwnd, &paths, true) {
                    mark_clipboard_dirty();
                    if let Some(first) = paths.first() {
                        let side = self.focused_split;
                        let explorer_state =
                            &mut self.active_tab_mut().view_mut(side).explorer_state;
                        explorer_state.selected_paths.clear();
                        explorer_state.selected_paths.insert(first.clone());
                    }
                }
            }
            ItemViewerContextAction::Copy(paths) => {
                if set_clipboard_files(self.hwnd, &paths, false) {
                    mark_clipboard_dirty();
                }
            }
            ItemViewerContextAction::CopyPath(paths) => {
                use crate::gui::utils::copy_text_to_clipboard;

                // Convert paths to strings and join with newlines
                let path_strings: Vec<String> =
                    paths.iter().map(|p| p.display().to_string()).collect();

                let text = path_strings.join("\r\n");
                let _ = copy_text_to_clipboard(self.hwnd, &text);
            }
            ItemViewerContextAction::Paste => {
                if let Err(e) = self.paste_clipboard_native() {
                    eprintln!("Paste failed: {}", e);
                }
            }
            ItemViewerContextAction::Restore(paths) => {
                let recycle_bin_pidls: Vec<Vec<u8>> = {
                    let view = self.active_tab().view(self.focused_split);
                    paths
                        .iter()
                        .filter_map(|path| {
                            view.files
                                .iter()
                                .find(|item| &item.path == path)
                                .and_then(|item| item.recycle_bin_pidl.clone())
                        })
                        .collect()
                };

                if let Err(e) = self.restore_paths_native(recycle_bin_pidls) {
                    eprintln!("Recycle bin restore failed: {:?}", e);
                }

                self.load_path();
            }
            ItemViewerContextAction::AddTag(paths) => {
                self.tags_state.open_picker(paths);
            }
            ItemViewerContextAction::RemoveTag(paths) => {
                if self.tags_state.remove_paths(&paths) {
                    self.persist_tags();
                }
            }
            ItemViewerContextAction::RenameRequest(path, new_name) => {
                let trimmed = new_name.trim();

                if trimmed.is_empty() {
                    self.rename_state = None;
                    return;
                }

                // Validate the new name for Windows filename rules
                if !Self::is_submitted_filename_valid(trimmed) {
                    // Don't cancel the rename, keep user in input state with error shown
                    if let Some(rename_state) = &mut self.rename_state {
                        rename_state.new_name = trimmed.to_string();
                        rename_state.should_focus = true;
                        rename_state.validation_error_show = true;
                    }
                    return;
                }

                if let Some(parent) = path.parent() {
                    let target = parent.join(trimmed);

                    // Avoid no-op rename
                    if path != target {
                        unsafe {
                            use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
                            use windows::Win32::UI::Shell::{
                                FOF_ALLOWUNDO, FileOperation, IFileOperation, IShellItem,
                                SHCreateItemFromParsingName,
                            };
                            use windows::core::HSTRING;

                            let file_op: IFileOperation =
                                CoCreateInstance(&FileOperation, None, CLSCTX_ALL).unwrap();

                            file_op.SetOperationFlags(FOF_ALLOWUNDO).ok();

                            let source_item: IShellItem = SHCreateItemFromParsingName(
                                &HSTRING::from(path.to_string_lossy().to_string()),
                                None,
                            )
                            .unwrap();

                            // Rename keeps same parent, so only pass new name
                            file_op
                                .RenameItem(&source_item, &HSTRING::from(trimmed), None)
                                .ok();

                            file_op.PerformOperations().ok();
                        }

                        // Queue the renamed file for auto-selection after refresh
                        let side = self.focused_split;
                        self.active_tab_mut()
                            .view_mut(side)
                            .explorer_state
                            .pending_selection_paths = Some(vec![target.clone()]);

                        if self.tags_state.remap_path_prefix(path.as_path(), &target) {
                            self.persist_tags();
                        }
                    }
                }

                self.rename_state = None;
                self.load_path();
            }

            ItemViewerContextAction::RenameCancel => {
                self.rename_state = None;
            }
            ItemViewerContextAction::Delete(paths) => {
                let allow_undo = !self.current_nav().is_recycle_bin();
                if let Err(e) = self.delete_paths_native(paths.clone(), allow_undo) {
                    eprintln!("Native delete failed: {:?}", e);

                    // fallback (rare, but safe)
                    for path in &paths {
                        self.delete_path(path);
                    }
                }

                let mut tags_changed = false;
                for path in &paths {
                    tags_changed |= self.tags_state.remove_path_prefix(path);
                }

                if tags_changed {
                    self.persist_tags();
                }

                self.load_path();
            }
            ItemViewerContextAction::Properties(paths) => {
                self.open_properties_multi(&paths);
            }
        }
    }

    pub fn paste_clipboard_native(&mut self) -> windows::core::Result<()> {
        use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
        use windows::Win32::UI::Shell::{
            FOF_ALLOWUNDO, FileOperation, IFileOperation, IShellItem, SHCreateItemFromParsingName,
        };
        use windows::core::HSTRING;

        let paths = match read_clipboard_files() {
            ClipboardFileRead::Files(paths) if !paths.is_empty() => paths,
            _ => return Ok(()),
        };

        if self.current_nav().is_recycle_bin() {
            return Ok(());
        }

        let is_cut = is_clipboard_cut();
        let target_dir = self.current_nav().current.clone();
        let before_entries = Self::directory_child_paths(&target_dir);

        unsafe {
            let file_op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)?;

            // Leave collision handling to the Windows shell. In particular, a folder with the
            // same name in the target should produce Explorer's merge confirmation instead of
            // silently creating a " - Copy" sibling.
            file_op.SetOperationFlags(FOF_ALLOWUNDO)?;

            let target_item: IShellItem = SHCreateItemFromParsingName(
                &HSTRING::from(self.current_nav().current.to_string_lossy().to_string()),
                None,
            )?;

            for path in &paths {
                let source_item: IShellItem = SHCreateItemFromParsingName(
                    &HSTRING::from(path.to_string_lossy().to_string()),
                    None,
                )?;

                if is_cut {
                    file_op.MoveItem(&source_item, &target_item, None, None)?;
                } else {
                    file_op.CopyItem(&source_item, &target_item, None, None)?;
                }
            }

            file_op.PerformOperations()?;
        }

        if is_cut && self.move_tagged_paths_to_dir(&paths, &target_dir) {
            self.persist_tags();
        }

        let pasted_paths = Self::selection_paths_after_paste(&target_dir, &before_entries, &paths);
        if !pasted_paths.is_empty() {
            let side = self.focused_split;
            self.active_tab_mut()
                .view_mut(side)
                .explorer_state
                .pending_selection_paths = Some(pasted_paths);
        }

        self.load_path();
        Ok(())
    }

    fn directory_child_paths(dir: &Path) -> HashSet<PathBuf> {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn selection_paths_after_paste(
        target_dir: &Path,
        before_entries: &HashSet<PathBuf>,
        sources: &[PathBuf],
    ) -> Vec<PathBuf> {
        let mut pasted_paths: Vec<PathBuf> = Self::directory_child_paths(target_dir)
            .difference(before_entries)
            .cloned()
            .collect();

        if pasted_paths.is_empty() {
            for source in sources {
                if let Some(name) = source.file_name() {
                    let candidate = target_dir.join(name);
                    if candidate.exists() {
                        pasted_paths.push(candidate);
                    }
                }
            }
        }

        pasted_paths
    }

    pub fn delete_path(&self, path: &PathBuf) {
        if !shell_delete_to_recycle_bin(path) {
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(path);
            } else {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    pub fn delete_paths_native(
        &self,
        paths: Vec<PathBuf>,
        allow_undo: bool,
    ) -> windows::core::Result<()> {
        use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
        use windows::Win32::UI::Shell::{
            FOF_ALLOWUNDO, FileOperation, IFileOperation, IShellItem, SHCreateItemFromParsingName,
        };
        use windows::core::HSTRING;

        unsafe {
            let file_op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)?;

            // Recycle-bin view needs permanent delete; normal view keeps undo.
            let flags = if allow_undo {
                FOF_ALLOWUNDO | FOF_WANTNUKEWARNING
            } else {
                FOF_WANTNUKEWARNING
            };
            file_op.SetOperationFlags(flags)?;

            for path in paths {
                let item: IShellItem = SHCreateItemFromParsingName(
                    &HSTRING::from(path.to_string_lossy().to_string()),
                    None,
                )?;

                file_op.DeleteItem(&item, None)?;
            }

            file_op.PerformOperations()?;
        }

        Ok(())
    }

    pub fn restore_paths_native(&self, pidls: Vec<Vec<u8>>) -> windows::core::Result<()> {
        use windows::Win32::System::Com::{CoTaskMemAlloc, CoTaskMemFree};

        unsafe {
            for pidl_bytes in pidls {
                let pidl = CoTaskMemAlloc(pidl_bytes.len()) as *mut u8;
                if pidl.is_null() {
                    return Err(Error::from(HRESULT(0x80004005u32 as i32)));
                }

                let result = (|| -> windows::core::Result<()> {
                    std::ptr::copy_nonoverlapping(pidl_bytes.as_ptr(), pidl, pidl_bytes.len());

                    let shell_item: IShellItem = SHCreateItemFromIDList(pidl as *const ITEMIDLIST)?;
                    let context_menu: IContextMenu =
                        shell_item.BindToHandler(None, &BHID_SFUIObject)?;

                    let restore_verb = PCSTR(b"undelete\0".as_ptr());
                    let info = CMINVOKECOMMANDINFOEX {
                        cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
                        fMask: 0,
                        hwnd: HWND(std::ptr::null_mut()),
                        lpVerb: restore_verb,
                        lpVerbW: PCWSTR::null(),
                        nShow: SW_SHOWNORMAL.0,
                        ..Default::default()
                    };

                    context_menu.InvokeCommand(&info as *const _ as *const CMINVOKECOMMANDINFO)
                })();

                CoTaskMemFree(Some(pidl as _));
                result?;
            }
        }

        Ok(())
    }

    pub fn open_properties_multi(&self, paths: &[PathBuf]) {
        if paths.is_empty() {
            return;
        }

        if paths.len() == 1 {
            self.open_properties(&paths[0]);
            return;
        }

        if let Some(data_object) = create_data_object(paths) {
            unsafe {
                let _ = SHMultiFileProperties(&data_object, 0);
            }
        }
    }

    pub fn open_properties(&self, path: &Path) {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let verb: Vec<u16> = OsStr::new("properties")
            .encode_wide()
            .chain(Some(0))
            .collect();

        unsafe {
            let mut info = SHELLEXECUTEINFOW {
                cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                fMask: SEE_MASK_INVOKEIDLIST,
                lpVerb: PCWSTR(verb.as_ptr()),
                lpFile: PCWSTR(wide.as_ptr()),
                nShow: SW_SHOW.0,
                ..Default::default()
            };
            let _ = ShellExecuteExW(&mut info);
        }
    }

    fn cleanup_resources(&mut self) {
        println!("Cleaning up resources...");

        // Tell background workers to exit.
        self.shutdown.store(true, Ordering::Relaxed);

        for tab in &mut self.tabs {
            for view in std::iter::once(&mut tab.primary_view).chain(tab.split_view.iter_mut()) {
                // Dropping the senders/receivers wakes any blocked workers.
                view.size_req_tx.take();
                view.size_rx.take();
                view.rx.take();

                // Wait for all workers to terminate.
                for handle in view.size_threads.drain(..) {
                    let _ = handle.join();
                }
            }
        }
    }

    pub fn handle_draw_settings_window(&mut self, ctx: &egui::Context, palette: &ThemePalette) {
        if let Some(action) =
            draw_settings_window(ctx, &mut self.settings_window, &mut self.i18n, palette)
        {
            match action {
                SettingsAction::ApplySettings => {
                    self.save_app_settings_to_disk();

                    if let Some(hwnd) = self.hwnd {
                        crate::gui::windows::windowsoverrides::set_window_mode(
                            hwnd,
                            &self.settings_window.current_settings.window_size_mode,
                        );
                    }
                }
                SettingsAction::ResetToDefaults => {
                    self.settings_window.current_settings = Default::default();
                    if let Some(hwnd) = self.hwnd {
                        crate::gui::windows::windowsoverrides::set_window_mode(
                            hwnd,
                            &self.settings_window.current_settings.window_size_mode,
                        );
                    }
                }
                SettingsAction::ResetFavourites => {
                    self.sidebar_state.favorites = self.default_favorites();
                    self.persist_favorites();
                }
            }
        }
    }

    pub fn handle_draw_about_window(&mut self, ctx: &egui::Context, palette: &ThemePalette) {
        // TODO: Implement about window
        draw_about_window(&mut self.i18n, ctx, &mut self.about_window, palette);
    }

    pub fn handle_tabbar_action(
        &mut self,
        tabbar_action: Option<ItemViewerNavBarAction>,
        drag_sources: Option<&[PathBuf]>,
    ) {
        if let Some(action) = tabbar_action.as_ref().and_then(|t| t.nav.as_ref()) {
            match action {
                ItemViewerNavAction::Back => {
                    let snapshot = directory_settings_snapshot_for_view(
                        self.active_tab().view(self.focused_split),
                    );
                    let _ = persist_directory_settings_snapshot(
                        &mut self.settings_window.current_settings.directory_settings,
                        snapshot,
                    );
                    // Store current path in navigation history before going back
                    if let Some(parent) = self.current_nav().get_parent() {
                        let current = self.current_nav().current.clone();
                        let side = self.focused_split;
                        self.active_tab_mut()
                            .view_mut(side)
                            .explorer_state
                            .navigation_history
                            .insert(parent, current);
                    }
                    self.current_nav_mut().go_back();
                }
                ItemViewerNavAction::Forward => {
                    let snapshot = directory_settings_snapshot_for_view(
                        self.active_tab().view(self.focused_split),
                    );
                    let _ = persist_directory_settings_snapshot(
                        &mut self.settings_window.current_settings.directory_settings,
                        snapshot,
                    );
                    self.current_nav_mut().go_forward();
                }
                ItemViewerNavAction::Up => {
                    let snapshot = directory_settings_snapshot_for_view(
                        self.active_tab().view(self.focused_split),
                    );
                    let _ = persist_directory_settings_snapshot(
                        &mut self.settings_window.current_settings.directory_settings,
                        snapshot,
                    );
                    // Store current path in navigation history before going up
                    if let Some(parent) = self.current_nav().get_parent() {
                        let current = self.current_nav().current.clone();
                        let side = self.focused_split;
                        self.active_tab_mut()
                            .view_mut(side)
                            .explorer_state
                            .navigation_history
                            .insert(parent, current);
                    }
                    self.current_nav_mut().go_up();
                }
            }
            self.mark_tab_infos_dirty();
            self.load_path();

            // Restore selection for Back and Up actions
            if matches!(action, ItemViewerNavAction::Back | ItemViewerNavAction::Up) {
                let current_path = self.current_nav().current.clone();
                let side = self.focused_split;
                let explorer_state = &mut self.active_tab_mut().view_mut(side).explorer_state;
                if let Some(last_visited) = explorer_state.navigation_history.get(&current_path) {
                    explorer_state.navigation_selection = Some(last_visited.clone());
                } else {
                    explorer_state.navigation_selection = None;
                }
                explorer_state.selection_anchor = None;
                explorer_state.selected_paths.clear();
                explorer_state.selection_focus = None;
            }
        } else {
            if let Some(path) = tabbar_action.as_ref().and_then(|t| t.nav_to.as_ref()) {
                let snapshot = directory_settings_snapshot_for_view(
                    self.active_tab().view(self.focused_split),
                );
                let _ = persist_directory_settings_snapshot(
                    &mut self.settings_window.current_settings.directory_settings,
                    snapshot,
                );
                // Store current path in navigation history before navigating
                if let Some(parent) = self.current_nav().get_parent() {
                    let current = self.current_nav().current.clone();
                    let side = self.focused_split;
                    self.active_tab_mut()
                        .view_mut(side)
                        .explorer_state
                        .navigation_history
                        .insert(parent, current);
                }

                self.current_nav_mut().go_to(path.clone());
                self.mark_tab_infos_dirty();
                self.load_path();
            }
            if tabbar_action
                .as_ref()
                .map(|t| t.refresh_current_directory)
                .unwrap_or(false)
            {
                self.load_path();
            }
            if let Some(target_dir) = tabbar_action
                .as_ref()
                .and_then(|t| t.move_files_to_breadcrumb_dir.as_ref())
            {
                if let Some(sources) = drag_sources {
                    self.move_selected_paths_to_dir(sources, target_dir.clone());
                }
            }
            if tabbar_action
                .as_ref()
                .map(|t| t.create_folder)
                .unwrap_or(false)
            {
                self.create_new_folder();
            }
            if tabbar_action
                .as_ref()
                .map(|t| t.create_file)
                .unwrap_or(false)
            {
                self.create_new_file();
            }
            if tabbar_action
                .as_ref()
                .map(|t| t.add_favorite)
                .unwrap_or(false)
            {
                self.add_favorite();
            }
            if tabbar_action
                .as_ref()
                .map(|t| t.remove_favorite)
                .unwrap_or(false)
            {
                let path = self.current_nav().current.clone();
                self.remove_favorite(&path);
            }
        }
    }

    pub fn handle_tabs_action(
        &mut self,
        tabs_action: Option<TabsAction>,
        drag_sources: Option<&[PathBuf]>,
    ) {
        if let Some(action) = tabs_action {
            if let Some(id) = action.activate {
                self.active_tab = self.tabs.iter().position(|t| t.id == id).unwrap();
                self.focused_split = SplitSide::Primary;
                let has_split = {
                    let tab = self.active_tab_mut();
                    tab.primary_view.item_viewer_filter_state.dirty = true;
                    if let Some(split) = tab.split_view.as_mut() {
                        split.item_viewer_filter_state.dirty = true;
                    }
                    tab.split_view.is_some()
                };
                self.pending_tab_scroll_id = Some(id);
                self.load_view(SplitSide::Primary);
                if has_split {
                    self.load_view(SplitSide::Secondary);
                }
            }
            if action.open_new {
                let cloned_nav = self.current_nav().clone();
                let (sort_column, sort_ascending) = {
                    let view = self.active_tab().view(self.focused_split);
                    (view.sort_column, view.sort_ascending)
                };
                let id = self.next_tab_id;
                self.next_tab_id += 1;
                self.tabs
                    .push(TabState::new(id, cloned_nav, sort_column, sort_ascending));
                let current_settings = self.settings_window.current_settings.clone();
                apply_directory_settings_to_view(
                    &mut self.tabs.last_mut().unwrap().primary_view,
                    &current_settings,
                );
                self.active_tab = self.tabs.len() - 1;
                self.focused_split = SplitSide::Primary;
                self.pending_tab_scroll_id = Some(id);
                self.mark_tab_infos_dirty();
                self.load_path();
            }
            if let Some(id) = action.close {
                if self.tabs.len() > 1 {
                    if let Some(idx) = self.tabs.iter().position(|t| t.id == id) {
                        if let Some(tab) = self.tabs.get(idx) {
                            let snapshot = directory_settings_snapshot_for_view(&tab.primary_view);
                            let _ = persist_directory_settings_snapshot(
                                &mut self.settings_window.current_settings.directory_settings,
                                snapshot,
                            );
                            if let Some(split) = tab.split_view.as_ref() {
                                let snapshot = directory_settings_snapshot_for_view(split);
                                let _ = persist_directory_settings_snapshot(
                                    &mut self.settings_window.current_settings.directory_settings,
                                    snapshot,
                                );
                            }
                        }
                        self.tabs.remove(idx);
                        if self.active_tab >= self.tabs.len() {
                            self.active_tab = self.tabs.len() - 1;
                        }
                        self.focused_split = SplitSide::Primary;
                        if let Some(active_id) = self.tabs.get(self.active_tab).map(|t| t.id) {
                            self.pending_tab_scroll_id = Some(active_id);
                        }
                        self.mark_tab_infos_dirty();
                        self.load_path();
                    }
                } else {
                    let snapshot = directory_settings_snapshot_for_view(&self.tabs[0].primary_view);
                    let _ = persist_directory_settings_snapshot(
                        &mut self.settings_window.current_settings.directory_settings,
                        snapshot,
                    );
                    let (
                        _folder_scanning_enabled,
                        _show_hidden_files_folders,
                        _show_item_viewer_icons,
                        _windows_context_menu_enabled,
                        _window_size_mode,
                        start_path,
                        _saved_theme,
                        _pinned_tabs,
                        _time_format_24h,
                        _sort_column,
                        _sort_ascending,
                        _language,
                        _date_style,
                        _item_viewer_file_column_order,
                        _item_viewer_drive_column_order,
                        _recycle_bin_column_order,
                        _item_viewer_file_column_sizes,
                        _item_viewer_drive_column_sizes,
                        _recycle_bin_column_sizes,
                        _directory_settings,
                    ) = load_app_settings();
                    self.tabs[0].primary_view.nav = Navigation::new(start_path);
                    self.tabs[0].split_view = None;
                    self.active_tab = 0;
                    self.focused_split = SplitSide::Primary;
                    self.mark_tab_infos_dirty();
                    self.load_path();
                }
            }
            if let Some(path) = action.toggle_pin {
                if self
                    .settings_window
                    .current_settings
                    .pinned_tabs
                    .iter()
                    .any(|p| p == &path)
                {
                    self.settings_window
                        .current_settings
                        .pinned_tabs
                        .retain(|p| p != &path);
                } else {
                    self.settings_window.current_settings.pinned_tabs.push(path);
                }

                self.save_app_settings_to_disk();

                self.mark_tab_infos_dirty();
            }
            if let Some(target_dir) = action.move_files_to_tab_dir.as_ref() {
                if let Some(sources) = drag_sources {
                    self.move_selected_paths_to_dir(sources, target_dir.clone());
                }
            }
            if action.scroll_left {
                self.tab_scroll_offset = (self.tab_scroll_offset - 180.0).max(0.0);
            }

            if action.scroll_right {
                self.tab_scroll_offset =
                    (self.tab_scroll_offset + 180.0).min(action.max_scroll_offset);
            }
        }
    }

    /// Toggles the active tab's split view: creates a second view (duplicating
    /// the primary view's directory+sort, with its own selection/filter/listing)
    /// if none exists, or clears it if one does.
    pub(crate) fn toggle_split_for_active_tab(&mut self) {
        let has_split = self.active_tab().split_view.is_some();
        if has_split {
            self.active_tab_mut().split_view = None;
            if self.focused_split == SplitSide::Secondary {
                self.focused_split = SplitSide::Primary;
            }
        } else {
            let tab = self.active_tab_mut();
            tab.split_view = Some(tab.primary_view.duplicate_as_new());
            self.focused_split = SplitSide::Secondary;
            self.load_view(SplitSide::Secondary);
        }
    }

    /// Opens `path` as the active tab's split view, creating the split if none
    /// exists yet, or replacing the current split target if one does.
    pub(crate) fn open_path_in_split(&mut self, path: PathBuf) {
        let mut new_view = self
            .active_tab()
            .view(SplitSide::Primary)
            .duplicate_as_new();
        new_view.nav = Navigation::new(path);
        let current_settings = self.settings_window.current_settings.clone();
        apply_directory_settings_to_view(&mut new_view, &current_settings);
        let tab = self.active_tab_mut();
        tab.split_view = Some(new_view);
        self.focused_split = SplitSide::Secondary;
        self.load_view(SplitSide::Secondary);
    }

    fn move_selected_paths_to_dir(&mut self, sources: &[PathBuf], target_dir: PathBuf) {
        if sources.is_empty() {
            return;
        }

        unsafe {
            let file_op: IFileOperation =
                CoCreateInstance(&FileOperation, None, CLSCTX_ALL).unwrap();

            file_op
                .SetOperationFlags(FOF_SIMPLEPROGRESS | FOF_ALLOWUNDO | FOFX_SHOWELEVATIONPROMPT)
                .ok();

            let target_item: IShellItem = SHCreateItemFromParsingName(
                &HSTRING::from(target_dir.to_string_lossy().to_string()),
                None,
            )
            .unwrap();

            for source in sources {
                let source_item: IShellItem = SHCreateItemFromParsingName(
                    &HSTRING::from(source.to_string_lossy().to_string()),
                    None,
                )
                .unwrap();

                file_op
                    .MoveItem(&source_item, &target_item, None, None)
                    .ok();
            }

            file_op.PerformOperations().ok();
        }

        {
            let side = self.focused_split;
            let explorer_state = &mut self.active_tab_mut().view_mut(side).explorer_state;
            explorer_state.selected_paths.clear();
            explorer_state.selection_anchor = None;
            explorer_state.selection_focus = None;
        }
        self.load_path();
    }

    pub fn handle_sidebar_action(
        &mut self,
        sidebar_action: Option<SidebarAction>,
        drag_sources: Option<&[PathBuf]>,
    ) {
        if let Some(action) = sidebar_action {
            if let Some((from, to)) = action.reorder {
                let len = self.sidebar_state.favorites.len();

                if from < len {
                    let item = self.sidebar_state.favorites.remove(from);

                    // Clamp target index AFTER removal
                    let mut target = to;

                    if to > from {
                        target -= 1;
                    }

                    target = target.min(self.sidebar_state.favorites.len());

                    self.sidebar_state.favorites.insert(target, item);
                }

                self.persist_favorites();
            }
            if let Some(path) = action.nav_to {
                // Store current path in navigation history before navigating
                if let Some(parent) = self.current_nav().get_parent() {
                    let current = self.current_nav().current.clone();
                    let side = self.focused_split;
                    self.active_tab_mut()
                        .view_mut(side)
                        .explorer_state
                        .navigation_history
                        .insert(parent, current);
                }

                self.current_nav_mut().go_to(path);
                self.mark_tab_infos_dirty();
                self.load_path();
            }
            if let Some(path) = action.open_new_tab {
                self.open_new_tab(path);
                self.load_path();
            }
            if let Some(path) = action.select_favorite {
                self.sidebar_state.item_clicked = Some(path);
            }
            if let Some(path) = action.remove_favorite {
                self.remove_favorite(&path);
            }
            if let Some(target_dir) = action.move_files_to_sidebar_dir.as_ref() {
                if let Some(sources) = drag_sources {
                    self.move_selected_paths_to_dir(sources, target_dir.clone());
                }
            }
        }
    }

    pub fn handle_topbar_action(&mut self, topbar_action: Option<TopbarAction>) {
        if let Some(action) = topbar_action {
            if action.toggle_theme {
                self.theme = match self.theme {
                    ThemeMode::Dark => ThemeMode::Light,
                    ThemeMode::Light => ThemeMode::Dark,
                };
                self.theme_dirty = true;

                // Save the theme setting
                self.save_app_settings_to_disk();
            }

            if action.customize_theme {
                self.theme_customizer.open = true;
            }

            if action.open_settings {
                self.settings_window.open = true;
            }

            if action.about {
                self.about_window.open = true;
            }

            if action.exit {
                if let Some(hwnd) = self.hwnd {
                    unsafe {
                        let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                    }
                }
            }
            if action.toggle_file_explorer {
                self.display_file_explorer = !self.display_file_explorer;
                let tab = self.active_tab_mut();
                tab.primary_view.drag_state.active = false;
                tab.primary_view.drag_state.start_pos = None;
                tab.primary_view.drag_state.source_items.clear();
                if let Some(split) = tab.split_view.as_mut() {
                    split.drag_state.active = false;
                    split.drag_state.start_pos = None;
                    split.drag_state.source_items.clear();
                }
                self.tags_state.drag_state = None;
            }
            if action.toggle_sidebar {
                self.sidebar_collapsed = !self.sidebar_collapsed;
            }
            if action.toggle_active_tab_split {
                self.toggle_split_for_active_tab();
            }
        }
    }

    fn global_shortcuts_disabled(&self, ctx: &egui::Context) -> bool {
        let topbar_menu_open = ctx.memory(|mem| {
            mem.data
                .get_temp::<bool>(egui::Id::new("topbar_hamburger_menu"))
                .unwrap_or(false)
        });

        let active_tab = self.active_tab();
        self.theme_customizer.open
            || self.settings_window.open
            || self.about_window.open
            || self.tags_state.picker.is_some()
            || self.tags_state.delete_confirmation.is_some()
            || self
                .rename_state
                .as_ref()
                .map(|state| state.validation_error_show)
                .unwrap_or(false)
            || self.sidebar_state.non_ntfs_popup_path.is_some()
            || active_tab
                .primary_view
                .explorer_state
                .non_ntfs_popup_path
                .is_some()
            || active_tab
                .split_view
                .as_ref()
                .map(|view| view.explorer_state.non_ntfs_popup_path.is_some())
                .unwrap_or(false)
            || topbar_menu_open
            || !self.display_file_explorer
    }

    fn enter_address_bar_edit_mode(&mut self) {
        let current_path = self.current_nav().current.clone();
        let side = self.focused_split;
        let view = self.active_tab_mut().view_mut(side);

        view.breadcrumb_path_editing = true;
        view.breadcrumb_path_buffer = current_path.to_string_lossy().to_string();
        view.breadcrumb_just_started_editing = false;
        view.breadcrumb_select_all_on_focus = true;
        view.breadcrumb_path_error = false;
        view.breadcrumb_path_error_animation_time = 0.0;
    }

    fn selected_path_for_rename(&self) -> Option<PathBuf> {
        let view = self.active_tab().view(self.focused_split);

        if let Some(focus_idx) = view.explorer_state.selection_focus
            && focus_idx < view.item_viewer_filter_state.cached_indices.len()
        {
            let file_idx = view.item_viewer_filter_state.cached_indices[focus_idx];
            let focused_path = view.files.get(file_idx)?.path.clone();
            if view.explorer_state.selected_paths.contains(&focused_path) {
                return Some(focused_path);
            }
        }

        let mut selected_paths: Vec<PathBuf> =
            view.explorer_state.selected_paths.iter().cloned().collect();
        selected_paths.sort();
        selected_paths.into_iter().next()
    }

    fn selected_paths_for_properties(&self) -> Option<Vec<PathBuf>> {
        let view = self.active_tab().view(self.focused_split);
        if view.explorer_state.selected_paths.is_empty() {
            return None;
        }

        let mut paths: Vec<PathBuf> = view.explorer_state.selected_paths.iter().cloned().collect();
        paths.sort();
        paths.dedup();
        Some(paths)
    }

    pub fn handle_global_shortcuts(&mut self, ctx: &egui::Context) {
        if self.global_shortcuts_disabled(ctx) {
            return;
        }

        let shortcuts = ctx.input(|input| {
            let ctrl = input.modifiers.ctrl;
            let alt = input.modifiers.alt;
            let shift = input.modifiers.shift;

            (
                ctrl && shift && input.key_pressed(egui::Key::Tab),
                ctrl && input.key_pressed(egui::Key::Tab),
                ctrl && input.key_pressed(egui::Key::T),
                ctrl && input.key_pressed(egui::Key::W),
                ctrl && shift && input.key_pressed(egui::Key::N),
                ctrl && input.key_pressed(egui::Key::R),
                input.key_pressed(egui::Key::F5),
                input.key_pressed(egui::Key::F1),
                alt && input.key_pressed(egui::Key::D),
                input.key_pressed(egui::Key::F2),
                alt && input.key_pressed(egui::Key::Enter),
            )
        });

        if shortcuts.0 {
            self.activate_tab_relative(-1);
            return;
        }

        if shortcuts.1 {
            self.activate_tab_relative(1);
            return;
        }

        if shortcuts.2 {
            let action = TabsAction {
                open_new: true,
                ..Default::default()
            };
            self.handle_tabs_action(Some(action), None);
            return;
        }

        if shortcuts.3 {
            let action = TabsAction {
                close: Some(self.active_tab().id),
                ..Default::default()
            };
            self.handle_tabs_action(Some(action), None);
            return;
        }

        if shortcuts.4 {
            self.create_new_folder();
            return;
        }

        if shortcuts.5 || shortcuts.6 {
            self.load_path();
            return;
        }

        if shortcuts.7 {
            self.toggle_fullscreen();
        }

        if shortcuts.8 {
            self.enter_address_bar_edit_mode();
            return;
        }

        if shortcuts.9 {
            if self.rename_state.is_none()
                && !self
                    .active_tab()
                    .view(self.focused_split)
                    .breadcrumb_path_editing
                && let Some(path) = self.selected_path_for_rename()
            {
                let action = ItemViewerAction::StartEdit(path);
                handle_pending_actions(Some(action), self);
            }
            return;
        }

        if shortcuts.10 {
            if !self
                .active_tab()
                .view(self.focused_split)
                .breadcrumb_path_editing
                && let Some(paths) = self.selected_paths_for_properties()
            {
                self.open_properties_multi(&paths);
            }
        }
    }

    fn activate_tab_relative(&mut self, delta: isize) {
        if self.tabs.is_empty() {
            return;
        }

        let len = self.tabs.len() as isize;
        let next = (self.active_tab as isize + delta).rem_euclid(len) as usize;
        let id = self.tabs[next].id;
        let action = TabsAction {
            activate: Some(id),
            ..Default::default()
        };
        self.handle_tabs_action(Some(action), None);
    }

    fn toggle_fullscreen(&mut self) {
        if let Some(hwnd) = self.hwnd {
            toggle_window_fullscreen(hwnd);
        }
    }

    pub fn handle_throttle_size_requests(&mut self, ctx: &egui::Context) {
        // Throttle size requests to keep UI responsive
        let should_pause =
            ctx.input(|i| i.pointer.any_down() || i.smooth_scroll_delta.y.abs() > 0.0);
        if should_pause {
            return;
        }
        self.handle_throttle_size_requests_for(SplitSide::Primary);
        if self.active_tab().split_view.is_some() {
            self.handle_throttle_size_requests_for(SplitSide::Secondary);
        }
    }

    fn handle_throttle_size_requests_for(&mut self, side: SplitSide) {
        let view = self.active_tab_mut().view_mut(side);
        if view.size_req_tx.is_none() {
            return;
        }
        for _ in 0..6 {
            match view.pending_size_queue.pop_front() {
                Some(path) => {
                    let _ = view.size_req_tx.as_ref().unwrap().send(path);
                }
                None => break,
            }
        }
    }

    pub fn handle_directory_size_updates(&mut self, ctx: &egui::Context) {
        let mut any_updated = self.handle_directory_size_updates_for(SplitSide::Primary);
        if self.active_tab().split_view.is_some() {
            any_updated |= self.handle_directory_size_updates_for(SplitSide::Secondary);
        }
        if any_updated {
            ctx.request_repaint();
        }
    }

    fn handle_directory_size_updates_for(&mut self, side: SplitSide) -> bool {
        if self.active_tab().view(side).size_rx.is_none() {
            return false;
        }

        let mut updated = false;
        for _ in 0..128 {
            let received = self
                .active_tab()
                .view(side)
                .size_rx
                .as_ref()
                .unwrap()
                .try_recv();
            match received {
                Ok((path, size, done)) => {
                    if done {
                        self.active_tab_mut()
                            .view_mut(side)
                            .pending_size_set
                            .remove(&path);
                    }
                    self.folder_sizes.insert(
                        path.clone(),
                        ItemViewerFolderSizeState { bytes: size, done },
                    );
                    let view = self.active_tab_mut().view_mut(side);
                    if let Some(item) = view.files.iter_mut().find(|f| f.path == path) {
                        item.file_size = Some(size);
                        updated = true;
                    }
                }
                Err(_) => break,
            }
        }

        if updated {
            let view = self.active_tab_mut().view_mut(side);
            sort_files_by_keys(&mut view.files, &view.sort_keys);
        }
        updated
    }

    pub fn handle_directory_batch_recieve(&mut self, ctx: &egui::Context) {
        let mut any_updated = self.handle_directory_batch_recieve_for(SplitSide::Primary);
        if self.active_tab().split_view.is_some() {
            any_updated |= self.handle_directory_batch_recieve_for(SplitSide::Secondary);
        }
        if any_updated {
            ctx.request_repaint();
        }
    }

    fn handle_directory_batch_recieve_for(&mut self, side: SplitSide) -> bool {
        if self.active_tab().view(side).rx.is_none() {
            return false;
        }

        let mut any_change = false;
        let mut batch = Vec::with_capacity(128);
        let mut disconnected = false;

        {
            let view = self.active_tab().view(side);
            let rx = view.rx.as_ref().unwrap();
            for _ in 0..128 {
                match rx.try_recv() {
                    Ok(item) => batch.push(item),
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }

        if !batch.is_empty() {
            let folder_scanning_enabled = self
                .settings_window
                .current_settings
                .folder_scanning_enabled;
            for item in batch.iter() {
                if item.is_dir && folder_scanning_enabled {
                    // Only set up folder size tracking if scanning is enabled
                    self.folder_sizes.entry(item.path.clone()).or_insert(
                        ItemViewerFolderSizeState {
                            bytes: 0,
                            done: false,
                        },
                    );
                    let view = self.active_tab_mut().view_mut(side);
                    if view.pending_size_set.insert(item.path.clone()) {
                        view.pending_size_queue.push_back(item.path.clone());
                    }
                }
            }

            let view = self.active_tab_mut().view_mut(side);
            view.files.extend(batch);
            sort_files_by_keys(&mut view.files, &view.sort_keys);
            any_change = true;
        }

        if disconnected {
            let view = self.active_tab_mut().view_mut(side);
            view.rx = None;
            view.is_loading = false;
            any_change = true;
        }

        any_change
    }
}

unsafe fn pwstr_to_string(pw: windows::core::PWSTR) -> String {
    let mut len = 0usize;
    let mut ptr = pw.0;
    while !ptr.is_null() && unsafe { *ptr } != 0 {
        len += 1;
        ptr = unsafe { ptr.add(1) };
    }
    let slice = unsafe { std::slice::from_raw_parts(pw.0, len) };
    String::from_utf16_lossy(slice)
}

fn split_parent(paths: &[PathBuf]) -> Option<(PathBuf, Vec<PathBuf>)> {
    if paths.is_empty() {
        return None;
    }

    let parent = paths[0].parent()?.to_path_buf();

    // Ensure all share same parent (Explorer requirement)
    if !paths.iter().all(|p| p.parent() == Some(parent.as_path())) {
        return None;
    }

    let children: Vec<PathBuf> = paths
        .iter()
        .filter_map(|p| p.file_name().map(PathBuf::from))
        .collect();

    Some((parent, children))
}

pub fn create_data_object(paths: &[PathBuf]) -> Option<IDataObject> {
    let (parent, children) = split_parent(paths)?;

    unsafe {
        // Parent PIDL
        let parent_w: Vec<u16> = parent.as_os_str().encode_wide().chain(Some(0)).collect();
        let parent_pidl = ILCreateFromPathW(PCWSTR(parent_w.as_ptr()));
        if parent_pidl.is_null() {
            return None;
        }

        let mut child_pidls: Vec<*const ITEMIDLIST> = Vec::new();
        let mut full_pidls: Vec<*mut ITEMIDLIST> = Vec::new();

        for child in children {
            let full = parent.join(child);
            let wide: Vec<u16> = full.as_os_str().encode_wide().chain(Some(0)).collect();

            let full_pidl = ILCreateFromPathW(PCWSTR(wide.as_ptr()));
            if !full_pidl.is_null() {
                // 🔥 Convert to relative PIDL
                let rel = ILFindLastID(full_pidl);
                child_pidls.push(rel);
                full_pidls.push(full_pidl);
            }
        }

        let result: Result<IDataObject> =
            SHCreateDataObject(Some(parent_pidl), Some(&child_pidls), None);

        for full_pidl in full_pidls {
            CoTaskMemFree(Some(full_pidl as _));
        }
        CoTaskMemFree(Some(parent_pidl as _));

        result.ok()
    }
}

pub fn calculate_folder_sizes_parallel(
    req_rx: Receiver<PathBuf>,
    done_tx: Sender<(PathBuf, u64, bool)>,
    shutdown: Arc<AtomicBool>,
    num_threads: usize,
) -> Vec<std::thread::JoinHandle<()>> {
    let req_rx = Arc::new(req_rx);
    let mut handles = Vec::with_capacity(num_threads);

    for _ in 0..num_threads {
        let rx = Arc::clone(&req_rx);
        let tx = done_tx.clone();
        let shutdown = Arc::clone(&shutdown);

        let handle = thread::spawn(move || {
            // Loop until channel closes or shutdown is triggered
            while !shutdown.load(Ordering::Relaxed) {
                // Use blocking recv; will wake immediately on a message or channel close
                let path = match rx.recv() {
                    Ok(p) => p,
                    Err(_) => break, // Channel closed
                };

                // Optional: check shutdown inside heavy work
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }

                parallel_directory_scan(path, tx.clone());

                // Optional: if parallel_directory_scan is very CPU heavy,
                // you can add a small yield:
                // thread::yield_now();
            }
        });

        handles.push(handle);
    }

    handles
}

pub fn handle_pending_actions(pending_action: Option<ItemViewerAction>, explorer: &mut MainWindow) {
    if let Some(action) = pending_action {
        let side = explorer.focused_split;
        let is_drive_view = explorer.current_nav().is_root();
        let is_recycle_bin_view = explorer.current_nav().is_recycle_bin();

        if is_recycle_bin_view {
            match &action {
                ItemViewerAction::CreateFolder
                | ItemViewerAction::CreateFile
                | ItemViewerAction::OpenTerminal
                | ItemViewerAction::Open(_)
                | ItemViewerAction::OpenWithDefault(_)
                | ItemViewerAction::OpenInNewTab(_)
                | ItemViewerAction::OpenInSplitView(_)
                | ItemViewerAction::StartEdit(_)
                | ItemViewerAction::FilesDropped(_)
                | ItemViewerAction::MoveItems { .. }
                | ItemViewerAction::Context(ItemViewerContextAction::Copy(_))
                | ItemViewerAction::Context(ItemViewerContextAction::CopyPath(_))
                | ItemViewerAction::Context(ItemViewerContextAction::Paste)
                | ItemViewerAction::Context(ItemViewerContextAction::AddTag(_))
                | ItemViewerAction::Context(ItemViewerContextAction::RemoveTag(_))
                | ItemViewerAction::Context(ItemViewerContextAction::RenameRequest(_, _))
                | ItemViewerAction::Context(ItemViewerContextAction::RenameCancel) => {
                    return;
                }
                _ => {}
            }
        }

        match action {
            ItemViewerAction::Sort {
                column,
                additive,
                remove,
            } => explorer.toggle_sort(column, additive, remove),
            ItemViewerAction::ToggleColumnVisibility(column) => {
                let new_order = {
                    let view = explorer.active_tab_mut().view_mut(side);
                    let column_state = &mut view.column_state;

                    if column_state.toggle_column_visibility(
                        is_drive_view,
                        is_recycle_bin_view,
                        column,
                    ) {
                        Some(
                            column_state
                                .order(is_drive_view, is_recycle_bin_view)
                                .to_vec(),
                        )
                    } else {
                        None
                    }
                };

                if let Some(order) = new_order {
                    explorer.apply_item_viewer_column_order(
                        side,
                        is_drive_view,
                        is_recycle_bin_view,
                        &order,
                    );
                    let snapshot =
                        directory_settings_snapshot_for_view(explorer.active_tab().view(side));
                    let _ = persist_directory_settings_snapshot(
                        &mut explorer.settings_window.current_settings.directory_settings,
                        snapshot,
                    );
                }
            }
            ItemViewerAction::FitColumn(column) => {
                let view = explorer.active_tab_mut().view_mut(side);

                view.column_state.pending_fit_request =
                    Some(ItemViewerColumnFitRequest::Column(column));

                view.column_state.layout_generation =
                    view.column_state.layout_generation.wrapping_add(1);
            }

            ItemViewerAction::FitAllColumns => {
                let view = explorer.active_tab_mut().view_mut(side);

                view.column_state.pending_fit_request = Some(ItemViewerColumnFitRequest::All);

                view.column_state.layout_generation =
                    view.column_state.layout_generation.wrapping_add(1);
            }
            ItemViewerAction::CreateFolder => explorer.create_new_folder(),
            ItemViewerAction::CreateFile => explorer.create_new_file(),
            ItemViewerAction::RefreshCurrentDirectory => {
                explorer.load_path();
            }
            ItemViewerAction::OpenTerminal => {
                let current_dir = explorer.current_nav().current.clone();
                open_default_terminal(&current_dir);
            }
            ItemViewerAction::MoveColumnLeft(column) => {
                let moved = {
                    let view = explorer.active_tab_mut().view_mut(side);
                    view.column_state
                        .move_column(is_drive_view, is_recycle_bin_view, column, -1)
                };
                if moved {
                    let order = explorer
                        .active_tab()
                        .view(side)
                        .column_state
                        .order(is_drive_view, is_recycle_bin_view)
                        .to_vec();
                    explorer.apply_item_viewer_column_order(
                        side,
                        is_drive_view,
                        is_recycle_bin_view,
                        &order,
                    );
                    let snapshot =
                        directory_settings_snapshot_for_view(explorer.active_tab().view(side));
                    let _ = persist_directory_settings_snapshot(
                        &mut explorer.settings_window.current_settings.directory_settings,
                        snapshot,
                    );
                }
            }
            ItemViewerAction::MoveColumnRight(column) => {
                let moved = {
                    let view = explorer.active_tab_mut().view_mut(side);
                    view.column_state
                        .move_column(is_drive_view, is_recycle_bin_view, column, 1)
                };
                if moved {
                    let order = explorer
                        .active_tab()
                        .view(side)
                        .column_state
                        .order(is_drive_view, is_recycle_bin_view)
                        .to_vec();
                    explorer.apply_item_viewer_column_order(
                        side,
                        is_drive_view,
                        is_recycle_bin_view,
                        &order,
                    );
                    let snapshot =
                        directory_settings_snapshot_for_view(explorer.active_tab().view(side));
                    let _ = persist_directory_settings_snapshot(
                        &mut explorer.settings_window.current_settings.directory_settings,
                        snapshot,
                    );
                }
            }
            ItemViewerAction::MoveColumnToStart(column) => {
                let moved = {
                    let view = explorer.active_tab_mut().view_mut(side);
                    view.column_state.move_column_to_edge(
                        is_drive_view,
                        is_recycle_bin_view,
                        column,
                        true,
                    )
                };
                if moved {
                    let order = explorer
                        .active_tab()
                        .view(side)
                        .column_state
                        .order(is_drive_view, is_recycle_bin_view)
                        .to_vec();
                    explorer.apply_item_viewer_column_order(
                        side,
                        is_drive_view,
                        is_recycle_bin_view,
                        &order,
                    );
                    let snapshot =
                        directory_settings_snapshot_for_view(explorer.active_tab().view(side));
                    let _ = persist_directory_settings_snapshot(
                        &mut explorer.settings_window.current_settings.directory_settings,
                        snapshot,
                    );
                }
            }
            ItemViewerAction::MoveColumnToEnd(column) => {
                let moved = {
                    let view = explorer.active_tab_mut().view_mut(side);
                    view.column_state.move_column_to_edge(
                        is_drive_view,
                        is_recycle_bin_view,
                        column,
                        false,
                    )
                };
                if moved {
                    let order = explorer
                        .active_tab()
                        .view(side)
                        .column_state
                        .order(is_drive_view, is_recycle_bin_view)
                        .to_vec();
                    explorer.apply_item_viewer_column_order(
                        side,
                        is_drive_view,
                        is_recycle_bin_view,
                        &order,
                    );
                    let snapshot =
                        directory_settings_snapshot_for_view(explorer.active_tab().view(side));
                    let _ = persist_directory_settings_snapshot(
                        &mut explorer.settings_window.current_settings.directory_settings,
                        snapshot,
                    );
                }
            }
            ItemViewerAction::Select(path) => {
                let idx = {
                    let view = explorer.active_tab().view(explorer.focused_split);
                    view.item_viewer_filter_state
                        .cached_indices
                        .iter()
                        .position(|&i| view.files[i].path == path)
                        .or_else(|| view.files.iter().position(|f| f.path == path))
                };

                let side = explorer.focused_split;
                let view = explorer.active_tab_mut().view_mut(side);
                view.explorer_state.selected_paths.insert(path.clone());
                if let Some(idx) = idx {
                    view.explorer_state.selection_anchor = Some(idx);
                    view.explorer_state.selection_focus = Some(idx);
                }
            }
            ItemViewerAction::Deselect(path) => {
                let side = explorer.focused_split;
                explorer
                    .active_tab_mut()
                    .view_mut(side)
                    .explorer_state
                    .selected_paths
                    .remove(&path);
            }
            ItemViewerAction::SelectAll => {
                let selected: Vec<PathBuf> = {
                    let view = explorer.active_tab().view(explorer.focused_split);
                    view.item_viewer_filter_state
                        .cached_indices
                        .iter()
                        .map(|&idx| view.files[idx].path.clone())
                        .collect()
                };
                let side = explorer.focused_split;
                let view = explorer.active_tab_mut().view_mut(side);
                view.explorer_state.selected_paths.clear();
                view.explorer_state.selected_paths.extend(selected);
            }
            ItemViewerAction::DeselectAll => {
                let side = explorer.focused_split;
                explorer
                    .active_tab_mut()
                    .view_mut(side)
                    .explorer_state
                    .selected_paths
                    .clear();
            }
            ItemViewerAction::RangeSelect(paths) => {
                // Set selection_focus to the edge of the range that is farthest from the anchor
                let new_focus = {
                    let view = explorer.active_tab().view(explorer.focused_split);
                    if let Some(anchor_idx) = view.explorer_state.selection_anchor {
                        if let (Some(first_path), Some(last_path)) = (paths.first(), paths.last()) {
                            // Check if we're in a filtered view
                            let is_filtered = (view.item_viewer_filter_state.active
                                && !view.item_viewer_filter_state.query.is_empty())
                                || view.item_viewer_filter_state.cached_indices.len()
                                    != view.files.len();

                            let (first_idx, last_idx) = if is_filtered {
                                // Use filtered indices
                                (
                                    view.item_viewer_filter_state
                                        .cached_indices
                                        .iter()
                                        .position(|&i| &view.files[i].path == first_path)
                                        .unwrap_or(anchor_idx),
                                    view.item_viewer_filter_state
                                        .cached_indices
                                        .iter()
                                        .position(|&i| &view.files[i].path == last_path)
                                        .unwrap_or(anchor_idx),
                                )
                            } else {
                                // Use original file indices (unfiltered view)
                                (
                                    view.files
                                        .iter()
                                        .position(|f| &f.path == first_path)
                                        .unwrap_or(anchor_idx),
                                    view.files
                                        .iter()
                                        .position(|f| &f.path == last_path)
                                        .unwrap_or(anchor_idx),
                                )
                            };

                            // If moving down, focus the last item; if moving up, focus the first item
                            Some(if anchor_idx <= first_idx {
                                last_idx // moved down
                            } else {
                                first_idx // moved up
                            })
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                };

                // Clear current selection and add all range-selected files
                let side = explorer.focused_split;
                let view = explorer.active_tab_mut().view_mut(side);
                view.explorer_state.selected_paths.clear();
                for path in &paths {
                    view.explorer_state.selected_paths.insert(path.clone());
                }
                if let Some(focus) = new_focus {
                    view.explorer_state.selection_focus = Some(focus);
                }
            }
            ItemViewerAction::Open(path) => {
                {
                    let side = explorer.focused_split;
                    let view = explorer.active_tab_mut().view_mut(side);
                    view.explorer_state.selected_paths.clear();
                    view.explorer_state.selected_paths.insert(path.clone());
                    view.item_viewer_filter_state.dirty = true;
                    view.item_viewer_filter_state.cached_indices.clear();
                }
                let snapshot = directory_settings_snapshot_for_view(
                    explorer.active_tab().view(explorer.focused_split),
                );
                let _ = persist_directory_settings_snapshot(
                    &mut explorer.settings_window.current_settings.directory_settings,
                    snapshot,
                );

                // Store current path in navigation history before navigating
                if let Some(parent) = explorer.current_nav().get_parent() {
                    let current = explorer.current_nav().current.clone();
                    let side = explorer.focused_split;
                    explorer
                        .active_tab_mut()
                        .view_mut(side)
                        .explorer_state
                        .navigation_history
                        .insert(parent, current);
                }

                explorer.current_nav_mut().go_to(path);
                explorer.mark_tab_infos_dirty();
                explorer.load_path();
            }
            ItemViewerAction::OpenWithDefault(paths) => {
                for path in paths {
                    let path_str = path.to_string_lossy().to_string();
                    let wide_path: Vec<u16> = OsStr::new(&path_str)
                        .encode_wide()
                        .chain(std::iter::once(0))
                        .collect();

                    unsafe {
                        let result = ShellExecuteW(
                            None,
                            PCWSTR::null(),
                            PCWSTR(wide_path.as_ptr()),
                            PCWSTR::null(),
                            PCWSTR::null(),
                            SW_SHOWNORMAL,
                        );

                        if result.0 <= std::ptr::null_mut() {
                            eprintln!("Failed to open file: {}", path.display());
                        }
                    }
                }
            }
            ItemViewerAction::OpenInNewTab(path) => {
                explorer.open_new_tab(path);
                explorer.load_path();
            }
            ItemViewerAction::OpenInSplitView(path) => {
                explorer.open_path_in_split(path);
            }
            ItemViewerAction::Context(action) => {
                explorer.handle_context_action(action);
            }
            ItemViewerAction::StartEdit(path) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.display().to_string());
                explorer.rename_state = Some(RenameState {
                    path,
                    new_name: name,
                    should_focus: true,
                    validation_error_show: false,
                });
            }
            ItemViewerAction::ReplaceSelection(path) => {
                let idx = {
                    let view = explorer.active_tab().view(explorer.focused_split);
                    // Check if we're in a filtered view
                    let is_filtered = view.item_viewer_filter_state.active
                        && !view.item_viewer_filter_state.query.is_empty();

                    if is_filtered {
                        // Use filtered indices
                        view.item_viewer_filter_state
                            .cached_indices
                            .iter()
                            .position(|&i| view.files[i].path == path)
                    } else {
                        // Use original file indices (unfiltered view)
                        view.files.iter().position(|f| f.path == path)
                    }
                };

                let side = explorer.focused_split;
                let view = explorer.active_tab_mut().view_mut(side);
                view.explorer_state.selected_paths.clear();
                view.explorer_state.selected_paths.insert(path.clone());
                if let Some(idx) = idx {
                    view.explorer_state.selection_anchor = Some(idx);
                    view.explorer_state.selection_focus = Some(idx);
                }
            }
            ItemViewerAction::FilesDropped(dropped_files) => {
                let valid_files: Vec<PathBuf> =
                    dropped_files.into_iter().filter(|p| p.exists()).collect();

                if valid_files.is_empty() {
                    return;
                }

                explorer.dropped_files = valid_files.clone();

                let current_path = explorer.current_nav().current.clone();

                if let Err(e) = show_copy_move_dialog(valid_files, &current_path) {
                    eprintln!("Failed to show copy/move dialog: {}", e);
                }

                // ✅ Defer refresh (important)
                explorer.dropped_files_pending_ui_refresh = true;
            }
            ItemViewerAction::MoveItems {
                sources,
                target_dir,
            } => {
                unsafe {
                    let file_op: IFileOperation =
                        CoCreateInstance(&FileOperation, None, CLSCTX_ALL).unwrap();

                    // Optional: show UI + allow TeraCopy hooks
                    file_op
                        .SetOperationFlags(
                            FOF_SIMPLEPROGRESS | FOF_ALLOWUNDO | FOFX_SHOWELEVATIONPROMPT,
                        )
                        .ok();

                    // Convert target dir to IShellItem
                    let target_item: IShellItem = SHCreateItemFromParsingName(
                        &HSTRING::from(target_dir.to_string_lossy().to_string()),
                        None,
                    )
                    .unwrap();

                    for source in &sources {
                        let source_item: IShellItem = SHCreateItemFromParsingName(
                            &HSTRING::from(source.to_string_lossy().to_string()),
                            None,
                        )
                        .unwrap();

                        file_op
                            .MoveItem(&source_item, &target_item, None, None)
                            .ok();
                    }

                    file_op.PerformOperations().ok();
                }

                if explorer.move_tagged_paths_to_dir(&sources, &target_dir) {
                    explorer.persist_tags();
                }

                {
                    let side = explorer.focused_split;
                    let view = explorer.active_tab_mut().view_mut(side);
                    view.explorer_state.selected_paths.clear();
                    view.explorer_state.selection_anchor = None;
                    view.explorer_state.selection_focus = None;
                }
                explorer.load_path();
            }
            ItemViewerAction::ColumnSizesChanged => {
                let snapshot =
                    directory_settings_snapshot_for_view(explorer.active_tab().view(side));
                let _ = persist_directory_settings_snapshot(
                    &mut explorer.settings_window.current_settings.directory_settings,
                    snapshot,
                );

                explorer.save_app_settings_to_disk();
            }
        }
    }
}

pub fn handle_draw_customizetheme_window(
    i18n: &I18n,
    ctx: &egui::Context,
    theme_customizer: &mut ThemeCustomizer,
    palette: &ThemePalette,
    current_mode: ThemeMode,
    theme_dirty: &mut bool,
) {
    if let Some(action) = draw_theme_customizer(&i18n, ctx, theme_customizer, palette) {
        match action {
            ThemeCustomizerAction::ThemeUpdated(mode) => {
                let updated = match mode {
                    ThemeMode::Dark => theme_customizer.dark_palette.clone(),
                    ThemeMode::Light => theme_customizer.light_palette.clone(),
                };
                apply_font_to_context(ctx, &updated);
                set_palette(mode, updated);
                save_theme_settings(
                    &theme_customizer.light_palette,
                    &theme_customizer.dark_palette,
                );

                if mode == current_mode {
                    *theme_dirty = true;
                }
            }
            ThemeCustomizerAction::ResetToDefaults(mode) => {
                let default = get_default_palette(mode);
                match mode {
                    ThemeMode::Dark => theme_customizer.dark_palette = default.clone(),
                    ThemeMode::Light => theme_customizer.light_palette = default.clone(),
                }
                if mode == current_mode {
                    apply_font_to_context(ctx, &default);
                }
                set_palette(mode, default);
                save_theme_settings(
                    &theme_customizer.light_palette,
                    &theme_customizer.dark_palette,
                );

                if mode == current_mode {
                    *theme_dirty = true;
                }
            }
            ThemeCustomizerAction::ExportTheme(mode) => {
                let palette_to_export = match mode {
                    ThemeMode::Dark => &theme_customizer.dark_palette,
                    ThemeMode::Light => &theme_customizer.light_palette,
                };

                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Theme JSON", &["json"])
                    .set_file_name(match mode {
                        ThemeMode::Dark => "eden_theme_dark.json",
                        ThemeMode::Light => "eden_theme_light.json",
                    })
                    .save_file()
                {
                    if let Ok(json) = serde_json::to_string_pretty(palette_to_export) {
                        let _ = std::fs::write(path, json);
                    }
                }
            }
            ThemeCustomizerAction::ImportTheme(mode) => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Theme JSON", &["json"])
                    .pick_file()
                {
                    if let Ok(json) = std::fs::read_to_string(path) {
                        if let Ok(imported) = serde_json::from_str::<ThemePalette>(&json) {
                            match mode {
                                ThemeMode::Dark => theme_customizer.dark_palette = imported.clone(),
                                ThemeMode::Light => {
                                    theme_customizer.light_palette = imported.clone()
                                }
                            }
                            if mode == current_mode {
                                apply_font_to_context(ctx, &imported);
                            }
                            set_palette(mode, imported);
                            save_theme_settings(
                                &theme_customizer.light_palette,
                                &theme_customizer.dark_palette,
                            );

                            if mode == current_mode {
                                *theme_dirty = true;
                            }
                        }
                    }
                }
            }
        }
    }
}
