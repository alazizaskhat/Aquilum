use super::*;
use aquilum_core::client_api::{ClientApiConfig, ClientApiField};

pub(super) fn backend(ctx: &Context) -> Vec<Block> {
    let defaults = ClientApiConfig {
        enabled: false,
        bind: "127.0.0.1:8790".parse().expect("loopback"),
        workspace: ctx.workspace.clone().unwrap_or_default(),
        aquilum_settings: PathBuf::new(),
        tokens_file: PathBuf::new(),
        allowed_origins: Vec::new(),
        requests_per_minute: 120,
        burst: 20,
    };
    let config = ctx.client_api_config.as_ref().unwrap_or(&defaults);
    let status = &ctx.client_api_status;
    let status_key = if status.busy {
        "changing"
    } else if status.running {
        "running"
    } else if status.error.is_some() {
        "failed"
    } else {
        "stopped"
    };
    let mut rows = vec![
        bound(
            t("settings.backend.enabled"),
            Some(t("settings.backend.enabledHint")),
            if status.busy {
                Control::Locked(config.enabled)
            } else {
                Control::Switch(config.enabled)
            },
            Setter::ClientApi(ClientApiField::Enabled),
        ),
        info(
            t("settings.backend.status"),
            None,
            Control::Status {
                text: t(&format!("settings.backend.{status_key}")),
                tone: if status.error.is_some() {
                    Tone::Danger
                } else {
                    Tone::Secondary
                },
            },
        ),
        info(
            t("settings.backend.access"),
            Some(t("settings.backend.accessHint")),
            Control::Status {
                text: t("settings.backend.readOnly"),
                tone: Tone::Muted,
            },
        ),
    ];
    if let Some(address) = &status.address {
        let url = format!("http://{address}/api");
        rows.push(info(
            t("settings.backend.localAddress"),
            Some(url.clone()),
            Control::Buttons(vec![Button::new(
                t("settings.backend.copyAddress"),
                Action::Copy(1000, url),
            )]),
        ));
    }
    rows.push(info(
        t("settings.backend.controls"),
        None,
        Control::Buttons(vec![
            Button::new(t("settings.backend.restart"), Action::BackendRestart)
                .enabled(!status.busy && (config.enabled || status.running)),
            Button::new(t("settings.backend.refresh"), Action::BackendRefresh),
        ]),
    ));
    if let Some(error) = &status.error {
        rows.push(info(
            t("settings.backend.lastError"),
            Some(error.clone()),
            Control::Status {
                text: t("settings.backend.needsAttention"),
                tone: Tone::Danger,
            },
        ));
    }
    let mut port = Number::new(f64::from(config.bind.port()), 1024.0, 65535.0, 1.0);
    port.commit = true;
    let mut rate = Number::new(f64::from(config.requests_per_minute), 1.0, 6000.0, 1.0);
    rate.commit = true;
    let mut burst = Number::new(f64::from(config.burst), 1.0, 100.0, 1.0);
    burst.commit = true;
    let connection = vec![
        bound(
            t("settings.backend.folder"),
            Some(t("settings.backend.folderHint")),
            Control::Text {
                value: crate::workspace::normalize(&config.workspace)
                    .to_string_lossy()
                    .into_owned(),
                commit: true,
            },
            Setter::ClientApi(ClientApiField::Workspace),
        ),
        info(
            t("settings.backend.chooseFolder"),
            None,
            Control::Buttons(vec![Button::new(
                t("settings.backend.browse"),
                Action::BackendChooseWorkspace,
            )
            .enabled(!status.busy)]),
        ),
        bound(
            t("settings.backend.port"),
            Some(t("settings.backend.portHint")),
            Control::Number(port),
            Setter::ClientApi(ClientApiField::Port),
        ),
        bound(
            t("settings.backend.rate"),
            Some(t("settings.backend.rateHint")),
            Control::Number(rate),
            Setter::ClientApi(ClientApiField::RequestsPerMinute),
        ),
        bound(
            t("settings.backend.burst"),
            None,
            Control::Number(burst),
            Setter::ClientApi(ClientApiField::Burst),
        ),
        bound(
            t("settings.backend.origins"),
            Some(t("settings.backend.originsHint")),
            Control::Text {
                value: config.allowed_origins.join(", "),
                commit: true,
            },
            Setter::ClientApi(ClientApiField::Origins),
        ),
    ];
    let active = ctx
        .client_api_clients
        .iter()
        .filter(|client| client.enabled)
        .count();
    let mut clients = vec![info(
        t("settings.backend.tokens"),
        Some(t("settings.backend.tokensHint")),
        Control::Buttons(vec![Button::new(
            t("settings.backend.createToken"),
            Action::BackendCreateToken,
        )]),
    )];
    if active == 0 {
        clients.push(info(
            t("settings.backend.noClients"),
            Some(t("settings.backend.noClientsHint")),
            Control::Status {
                text: "0".into(),
                tone: Tone::Muted,
            },
        ));
    }
    for client in &ctx.client_api_clients {
        let control = if client.enabled {
            Control::Buttons(vec![Button::new(
                t("settings.backend.revoke"),
                Action::BackendRevokeToken(client.name.clone()),
            )])
        } else {
            Control::Status {
                text: t("settings.backend.revoked"),
                tone: Tone::Muted,
            }
        };
        clients.push(info(
            client.name.clone(),
            Some(t(if client.enabled {
                "settings.backend.clientActive"
            } else {
                "settings.backend.clientRevoked"
            })),
            control,
        ));
    }
    if let Some(path) = &status.notice {
        clients.push(info(
            t("settings.backend.tokenSaved"),
            Some(path.clone()),
            Control::Status {
                text: t("settings.backend.saved"),
                tone: Tone::Secondary,
            },
        ));
    }
    vec![
        Block {
            title: t("settings.backend.section"),
            rows,
        },
        Block {
            title: t("settings.backend.connection"),
            rows: connection,
        },
        Block {
            title: t("settings.backend.clients"),
            rows: clients,
        },
    ]
}
