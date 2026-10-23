use super::*;
use aquilum_core::connections::{Status, PAGE, PREVIEW};

fn button(key: &str, action: Action, enabled: bool) -> Button {
    Button::new(t(&format!("settings.connections.{key}")), action).enabled(enabled)
}

pub(super) fn connections(ctx: &Context) -> Vec<Block> {
    let s: &Status = &ctx.connections;
    let idle = !s.busy;
    let status = if s.busy {
        "working"
    } else if s.connected {
        "connected"
    } else {
        "disconnected"
    };
    if let Some(note) = &s.preview {
        let mut rows = vec![
            info(
                note.path.clone(),
                Some(t("settings.connections.readOnly")),
                Control::Buttons(vec![
                    button("back", Action::ConnectionBack, idle),
                    button(
                        "copy",
                        Action::ConnectionCopy(Some(note.path.clone())),
                        idle && s.connected && ctx.workspace.is_some(),
                    ),
                ]),
            ),
            info(
                t(&format!("settings.connections.{status}")),
                None,
                Control::Buttons(vec![
                    button(
                        "refresh",
                        Action::ConnectionRead(note.path.clone(), s.preview_offset),
                        idle,
                    ),
                    button(
                        "previous",
                        Action::ConnectionRead(
                            note.path.clone(),
                            s.preview_offset.saturating_sub(PREVIEW),
                        ),
                        idle && s.preview_offset > 0,
                    ),
                    button(
                        "next",
                        Action::ConnectionRead(note.path.clone(), note.next.unwrap_or(0)),
                        idle && note.next.is_some(),
                    ),
                    button("disconnect", Action::ConnectionDisconnect, true),
                ]),
            ),
        ];
        rows.extend(operation_rows(s));
        rows.push(info(
            String::new(),
            None,
            Control::Status {
                text: note.content.clone(),
                tone: Tone::Secondary,
            },
        ));
        return vec![Block {
            title: t("settings.connections.preview"),
            rows,
        }];
    }
    let mut rows = vec![
        bound(
            t("settings.connections.address"),
            Some(t("settings.connections.addressHint")),
            Control::Text {
                value: s.config.address.clone(),
                commit: true,
            },
            Setter::ConnectionAddress,
        ),
        info(
            t("settings.connections.token"),
            s.config
                .token_file
                .as_ref()
                .map(|p| p.display().to_string()),
            Control::Buttons(vec![button("chooseToken", Action::ConnectionToken, idle)]),
        ),
        info(
            t("settings.connections.status"),
            Some(t(&format!("settings.connections.{status}"))),
            Control::Buttons(vec![
                button("connect", Action::ConnectionBrowse(0), idle),
                button("disconnect", Action::ConnectionDisconnect, true),
            ]),
        ),
        info(
            t("settings.connections.copyAll"),
            Some(t("settings.connections.copyHint")),
            Control::Buttons(vec![button(
                "copyAll",
                Action::ConnectionCopy(None),
                idle && s.connected && ctx.workspace.is_some(),
            )]),
        ),
    ];
    if ctx
        .workspace
        .as_deref()
        .is_some_and(|root| aquilum_core::connections::legacy_count(root) > 0)
    {
        rows.push(info(
            t("settings.connections.organize"),
            Some(t("settings.connections.organizeHint")),
            Control::Buttons(vec![button("organize", Action::ConnectionOrganize, idle)]),
        ));
    }
    rows.extend(operation_rows(s));
    let mut blocks = vec![Block {
        title: t("settings.connections.section"),
        rows,
    }];
    if s.connected || !s.notes.is_empty() {
        let mut rows = vec![bound(
            t("settings.connections.filter"),
            None,
            Control::Text {
                value: s.query.clone(),
                commit: true,
            },
            Setter::ConnectionQuery,
        )];
        rows.push(info(
            t_with(
                "settings.connections.count",
                &[("count", &s.total.to_string())],
            ),
            None,
            Control::Buttons(vec![
                button(
                    "previous",
                    Action::ConnectionBrowse(s.offset.saturating_sub(PAGE)),
                    idle && s.offset > 0,
                ),
                button(
                    "next",
                    Action::ConnectionBrowse(s.next.unwrap_or(0)),
                    idle && s.next.is_some(),
                ),
            ]),
        ));
        for path in &s.notes {
            rows.push(info(
                path.clone(),
                None,
                Control::Buttons(vec![
                    button("read", Action::ConnectionRead(path.clone(), 0), idle),
                    button(
                        "copy",
                        Action::ConnectionCopy(Some(path.clone())),
                        idle && ctx.workspace.is_some(),
                    ),
                ]),
            ));
        }
        blocks.push(Block {
            title: t("settings.connections.notes"),
            rows,
        });
    }
    blocks
}

fn operation_rows(s: &Status) -> Vec<Row> {
    let mut rows = Vec::new();
    if let Some(error) = &s.error {
        rows.push(info(
            t("settings.connections.error"),
            None,
            Control::Status {
                text: error.clone(),
                tone: Tone::Danger,
            },
        ));
    }
    if let Some(destination) = &s.destination {
        rows.push(info(
            t_with(
                "settings.connections.copied",
                &[("count", &s.copied.to_string())],
            ),
            Some(format!(
                "{}\n{}",
                destination.display(),
                t_with(
                    "settings.connections.copyDetails",
                    &[
                        ("skipped", &s.skipped.to_string()),
                        ("conflicts", &s.conflicts.to_string()),
                        ("folders", &s.organized.to_string()),
                    ]
                )
            )),
            Control::Buttons(vec![button(
                "showFolder",
                Action::ConnectionShowFolder,
                true,
            )]),
        ));
    }
    rows
}
