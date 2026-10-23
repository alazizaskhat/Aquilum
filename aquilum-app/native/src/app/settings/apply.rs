use super::*;

impl App {
    pub(super) fn press(&mut self, root: &mut RenderRoot, dialog: &mut SettingsDialog, action: Action) -> Effect {
        let core = &self.workspace.core;
        let workspace = self.tree.root().map(Path::to_path_buf);
        match action {
            Action::ConnectionToken => {
                if let Some(path) = rfd::FileDialog::new().set_title(t("settings.connections.chooseToken")).add_filter("Aquilum token", &["token"]).pick_file() {
                    core.connections.configure(|config| config.token_file = Some(path));
                }
                Effect::Content
            }
            Action::ConnectionBrowse(offset) => {
                core.connections.start(core, aquilum_core::connections::Job::Browse(offset));
                Effect::Content
            }
            Action::ConnectionRead(path, offset) => {
                dialog.content.edit(root, |mut area| crate::ui::scroll::ScrollArea::set_offset(&mut area, 0.0));
                core.connections.start(core, aquilum_core::connections::Job::Read(path, offset));
                Effect::Content
            }
            Action::ConnectionBack => {
                core.connections.back();
                dialog.content.edit(root, |mut area| crate::ui::scroll::ScrollArea::set_offset(&mut area, 0.0));
                Effect::Content
            }
            Action::ConnectionCopy(note) => {
                if let Some(workspace) = workspace {
                    core.connections.start(core, aquilum_core::connections::Job::Copy { workspace, note });
                }
                Effect::Content
            }
            Action::ConnectionOrganize => {
                if let Some(workspace) = workspace {
                    core.connections.start(core, aquilum_core::connections::Job::Organize { workspace });
                }
                Effect::Content
            }
            Action::ConnectionDisconnect => { core.connections.disconnect(); Effect::Content }
            Action::ConnectionShowFolder => {
                if let Some(path) = core.connections.status().destination { system::open(&path.to_string_lossy()); }
                Effect::None
            }
            Action::BackendRestart => {
                if let Err(error) = core.client_api.request_apply(core) { core.client_api.record_error(error); }
                Effect::Content
            }
            Action::BackendRefresh => Effect::Content,
            Action::BackendChooseWorkspace => {
                let config = core.client_api.settings(workspace.as_deref());
                let mut picker = rfd::FileDialog::new().set_title(t("settings.backend.chooseFolder"));
                if config.workspace.is_dir() { picker = picker.set_directory(&config.workspace); }
                if let Some(path) = picker.pick_folder() {
                    let _ = core.client_api.configure(core, |config| config.workspace = path);
                }
                Effect::Content
            }
            Action::BackendCreateToken => {
                let filename = format!("device-{}.token", chrono::Local::now().format("%Y%m%d-%H%M%S"));
                let directory = core.client_api.config_path().parent().unwrap_or(Path::new("."));
                let _ = std::fs::create_dir_all(directory);
                if let Some(path) = rfd::FileDialog::new().set_title(t("settings.backend.saveToken")).set_directory(directory).set_file_name(&filename).save_file() {
                    let stem = path.file_stem().and_then(|name| name.to_str()).unwrap_or("device");
                    let name = if !stem.is_empty() && stem.len() <= 64 && stem.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte)) {
                        stem.to_owned()
                    } else { format!("device-{}", uuid::Uuid::new_v4().simple()) };
                    let _ = core.client_api.create_token(&name, &path);
                }
                Effect::Content
            }
            Action::BackendRevokeToken(name) => {
                let _ = core.client_api.revoke_token(&name);
                Effect::Content
            }
            Action::OpenTrash => {
                if let Some(workspace) = workspace {
                    match ensure_trash_impl(&workspace) {
                        Ok(path) => system::open(&path),
                        Err(error) => report!("корзина не открыта: {error:?}"),
                    }
                }
                Effect::None
            }
            Action::Wipe(wipe) => {
                let (layer, confirm) = ConfirmDialog::new(&t(&wipe.key("clearTitle")), &t(&wipe.key("clearDescription")), &t("common.cancel"), &t(&wipe.key("clearConfirm")));
                root.add_layer(layer, Point::ORIGIN);
                root.focus_on(Some(confirm.layer));
                dialog.confirm = Some((confirm, wipe));
                Effect::None
            }
            Action::SyncConnect => {
                match core.sync.connect(core) {
                    Ok(url) => system::open_url(&url),
                    Err(error) => report!("вход в Dropbox не начат: {error}"),
                }
                Effect::Content
            }
            Action::SyncDisconnect => {
                if let Err(error) = core.sync.disconnect() {
                    report!("Dropbox не отключён до конца: {error}");
                }
                Effect::Content
            }
            Action::SyncAllowDeletions => {
                core.sync.allow_deletions();
                Effect::Content
            }
            Action::Trash(path) => {
                let Some(workspace) = workspace else { return Effect::None };
                if let Err(error) = gate::trash(core, &workspace, &workspace.join(path)) {
                    report!("файл не убран в корзину: {error:?}");
                }
                Effect::Content
            }
            Action::SyncSend(paths) => {
                core.sync.allow(paths);
                Effect::Content
            }
            Action::Reveal(path) => {
                if let Some(workspace) = workspace {
                    system::reveal(&workspace.join(path));
                }
                Effect::None
            }
            Action::Restore(id) => {
                let Some(workspace) = workspace else { return Effect::None };
                if let Err(error) = gate::restore_deletion(core, &workspace, &id) {
                    report!("заметка не восстановлена: {error:?}");
                }
                Effect::Content
            }
            Action::TrashPage(page) => {
                dialog.trash_page = page;
                Effect::Content
            }
            Action::CreateTemplates => {
                let Some(workspace) = workspace else { return Effect::None };
                crate::app::templates::write_starters(core, &schema::templates_path(&workspace, &core.settings.get_config().templates.folder));
                Effect::Content
            }
            Action::CheckUpdates => {
                self.updater.check();
                Effect::Content
            }
            Action::InstallUpdate => {
                self.updater.install();
                Effect::Content
            }
            Action::NewToken => self.apply(Setter::Config(|c, _| c.mcp.token = schema::new_token()), Value::Bool(true)),
            Action::Copy(index, code) => {
                if crate::clipboard::copy(code) {
                    dialog.copied = Some(index);
                }
                Effect::Content
            }
            Action::OpenUrl(url) => {
                system::open_url(url);
                Effect::None
            }
        }
    }

    pub(super) fn apply(&mut self, setter: Setter, value: Value) -> Effect {
        match setter {
            Setter::ConnectionAddress => {
                if let Value::Text(address) = value { self.workspace.core.connections.configure(|config| config.address = address.trim().to_owned()); }
                Effect::Content
            }
            Setter::ConnectionQuery => {
                if let Value::Text(query) = value {
                    let core = &self.workspace.core;
                    core.connections.query(query);
                    core.connections.start(core, aquilum_core::connections::Job::Browse(0));
                }
                Effect::Content
            }
            Setter::ClientApi(field) => {
                use aquilum_core::client_api::ClientApiField;
                let core = &self.workspace.core;
                let workspace = self.tree.root().map(Path::to_path_buf);
                let _ = core.client_api.configure(core, |config| {
                    if config.workspace.as_os_str().is_empty() && let Some(workspace) = workspace { config.workspace = workspace; }
                    match (field, value) {
                        (ClientApiField::Enabled, Value::Bool(enabled)) => config.enabled = enabled,
                        (ClientApiField::Port, Value::Number(port)) => config.bind.set_port(port as u16),
                        (ClientApiField::Workspace, Value::Text(path)) => config.workspace = Path::new(path.trim()).to_path_buf(),
                        (ClientApiField::RequestsPerMinute, Value::Number(limit)) => config.requests_per_minute = limit as u32,
                        (ClientApiField::Burst, Value::Number(limit)) => config.burst = limit as u32,
                        (ClientApiField::Origins, Value::Text(origins)) => config.allowed_origins = origins.split(|ch: char| ch == ',' || ch.is_whitespace()).filter(|origin| !origin.is_empty()).map(str::to_owned).collect(),
                        _ => {},
                    }
                });
                Effect::Content
            }
            Setter::Scale => {
                let Value::Number(percent) = value else { return Effect::None };
                if self.interface.set_scale(percent / 100.0) { Effect::Rescale } else { Effect::None }
            }
            Setter::Autostart => {
                let Value::Bool(on) = value else { return Effect::None };
                if !crate::autostart::set(on) {
                    report!("автозапуск не изменён");
                }
                Effect::Content
            }
            Setter::HomePage => {
                let (Value::Text(page), Some(root)) = (value, self.tree.root()) else { return Effect::None };
                if let Err(error) = self.workspace.core.ui_state.set_home_page(&root.to_string_lossy(), page.trim()) {
                    report!("домашняя страница не сохранена: {error:?}");
                }
                self.refresh_home();
                Effect::None
            }
            Setter::VaultSync(set) => {
                let Some(root) = self.tree.root().map(Path::to_path_buf) else { return Effect::None };
                let core = &self.workspace.core;
                let mut sync = core.ui_state.workspace(&root).map(|known| known.sync).unwrap_or_default();
                set(&mut sync, &value);
                if let Err(error) = core.ui_state.set_sync(&root.to_string_lossy(), &sync) {
                    report!("настройки синхронизации базы не сохранены: {error:?}");
                }
                self.bind_sync();
                Effect::Content
            }
            Setter::Config(set) => {
                let core = &self.workspace.core;
                let before = core.settings.get_config();
                let mut after = before.clone();
                set(&mut after, &value);
                let language_changed = before.ui.language != after.ui.language;
                let colors_changed = before.ui.theme != after.ui.theme || before.ui.primary_color != after.ui.primary_color;
                let fonts_changed = before.ui.font.font_family != after.ui.font.font_family
                    || before.ui.font.font_weight != after.ui.font.font_weight
                    || before.ui.font.font_size_base != after.ui.font.font_size_base
                    || before.editor.font.font_family != after.editor.font.font_family
                    || before.editor.font.font_weight != after.editor.font.font_weight
                    || before.editor.font.font_size_base != after.editor.font.font_size_base
                    || before.editor.line_height != after.editor.line_height
                    || before.editor.max_width_ch != after.editor.max_width_ch;
                let mcp_changed = serde_json::to_value(&before.mcp).ok() != serde_json::to_value(&after.mcp).ok();
                let retention = before.trash.retention_days != after.trash.retention_days || before.history.retention_days != after.history.retention_days;
                let json = |c: &aquilum_core::settings::models::AppConfig| serde_json::to_value((&c.analysis, &c.search)).ok();
                let analysis_changed = json(&before) != json(&after);
                let methods_toggled = crate::links::Method::enabled(&before).is_empty() != crate::links::Method::enabled(&after).is_empty();
                crate::ui::motion::set(after.ui.animations);
                let rows_changed = before.ui.animations != after.ui.animations
                    || before.analysis.enable_bm25f != after.analysis.enable_bm25f
                    || before.updates.auto != after.updates.auto
                    || before.templates.folder != after.templates.folder
                    || mcp_changed
                    || retention;
                if let Err(error) = core.settings.update_config(after) {
                    report!("настройки не сохранены: {error}");
                    return Effect::None;
                }
                if mcp_changed {
                    core.apply_mcp_settings();
                }
                if retention {
                    self.cleanup_retention();
                }
                if language_changed || methods_toggled {
                    Effect::Everything
                } else if fonts_changed || colors_changed {
                    Effect::Restyle(fonts_changed)
                } else if rows_changed {
                    Effect::Content
                } else if analysis_changed {
                    Effect::Panel
                } else {
                    Effect::None
                }
            }
        }
    }

    pub(super) fn refresh_analysis(&mut self, root: &mut RenderRoot) {
        if self.panel_mode() == crate::links::Mode::Analysis {
            self.request_panel(root);
        }
    }
}
