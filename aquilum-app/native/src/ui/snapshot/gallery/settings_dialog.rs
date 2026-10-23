use aquilum_core::settings::models::AppConfig;
use masonry::app::RenderRoot;
use masonry::kurbo::Point;

use super::Scene;
use crate::ui::settings::schema::{Context, Section};
use crate::ui::settings;
use crate::ui::snapshot::shell_with;

fn show(root: &mut RenderRoot, config: &AppConfig, section: Section) {
    let (layer, _) = settings::dialog(config, &Context::new(1.0), section);
    root.add_layer(layer, Point::ORIGIN);
}

pub(super) fn primary(scene: &Scene, config: &AppConfig) {
    let (mut root, _) = scene.shell();
    show(&mut root, config, Section::Ui);
    scene.save(&mut root, "primary");
}

pub(super) fn snapshots(scene: &Scene, config: &AppConfig) {
    for scale in [1.75, 1.75 * 1.05] {
        let (mut root, _) = shell_with(scene.vault, scene.data, scale, scale);
        show(&mut root, config, Section::Ui);
        scene.save(&mut root, &format!("scaled-{}", (scale * 100.0).round()));
    }
    for section in [Section::Ui, Section::Editor, Section::Shortcuts, Section::Mcp, Section::System, Section::Trash] {
        let (mut root, _) = scene.shell();
        show(&mut root, config, section);
        scene.save(&mut root, &format!("settings-{}", section.key()));
    }
}

#[test]
#[ignore = "backend settings visual review"]
fn snapshot_backend_settings() {
    use aquilum_core::client_api::{ClientApiStatus, ClientToken};
    let _theme = crate::ui::theme::test_lock();
    let vault = crate::test_dir::tempdir();
    let data = crate::test_dir::tempdir();
    for language in ["en", "ru"] {
        crate::i18n::set_language(language);
        for (mode, suffix) in [("light", "light"), ("dark", "dark")] {
            crate::ui::theme::apply_setting(mode, false, "");
            let (mut root, _) = crate::ui::snapshot::shell(vault.path(), data.path());
            let mut ctx = Context::new(1.0);
            ctx.client_api_status = ClientApiStatus { configured: true, running: true, address: Some("127.0.0.1:8790".into()), ..Default::default() };
            ctx.client_api_clients = vec![ClientToken { name: "phone".into(), enabled: true }];
            ctx.client_api_config = Some(aquilum_core::client_api::ClientApiConfig {
                enabled: true, bind: "127.0.0.1:8790".parse().unwrap(), workspace: std::path::PathBuf::from("C:/Users/alazi/Documents/notes"),
                aquilum_settings: Default::default(), tokens_file: Default::default(), allowed_origins: vec!["https://test1.salemsmart.kz".into()], requests_per_minute: 120, burst: 20,
            });
            let (layer, dialog) = settings::dialog(&AppConfig::default(), &ctx, Section::Backend);
            root.add_layer(layer, Point::ORIGIN);
            root.run_rewrite_passes();
            root.handle_window_event(masonry::core::WindowEvent::AnimFrame(std::time::Duration::from_secs(1)));
            crate::ui::snapshot::save(&mut root, &format!("backend-settings-{language}-{suffix}"));
            for (offset, part) in [(500.0, "connection"), (10000.0, "clients")] {
                dialog.content.edit(&mut root, |mut scroll| crate::ui::scroll::ScrollArea::set_offset(&mut scroll, offset));
                crate::ui::snapshot::save(&mut root, &format!("backend-settings-{language}-{suffix}-{part}"));
            }
        }
    }
}
