//! The mission-control dashboard.
//!
//! The layout mirrors the ADW Cockpit Pi extension
//! (`.pi/extensions/adw-cockpit/index.ts`, `renderDashboard`) line for line,
//! translated from terminal box-drawing to Rune primitives:
//!
//! - a heading line: bold accent name at the left, `updated HH:MM:SS  ● live`
//!   at the right;
//! - a top band of two equal panels side by side — `1. OVERVIEW` and
//!   `2. MISSION` (the cockpit's `2. LATEST RUN`) — stacking when the window is
//!   too narrow, exactly as the cockpit stacks below 84 columns;
//! - `3. PIPELINE`: a segmented completion meter with `done/total` and a
//!   current-phase tail, over a wrapping grid of named status dots
//!   (done / current / pending), the cockpit's `phaseGrid`;
//! - numbered, titled, sharp-cornered boxes with a right-aligned tag in the
//!   title row;
//! - `key : value` rows: dim padded key, dim colon, status-coloured mono value;
//! - a slim `last …` status line under the panels.
//!
//! Read-only: no element in this file mutates the ledger. The only handlers are
//! a refresh, a theme cycle, an auto-refresh toggle and mission selection.

use rune::tok::{self, Theme, motion, space, type_};
use rune::{Color, Cx, El, ScrollSpec, col, el, row, text};

use crate::ledger::{CliEcho, HAPPY_PATH, MissionCard};
use crate::{Ui, refresh_now, select_next, select_prev, toggle_appearance, toggle_auto};

/// Everything a component needs to paint itself.
#[derive(Clone, Copy)]
struct Paint {
    t: Theme,
    hair: f32,
    /// The box-drawing colour: the cockpit's `border`.
    border: Color,
    dim: Color,
    /// Done, healthy, verified. The cockpit's `success`.
    ok: Color,
    /// The accent: titles, names, ordinary signal.
    info: Color,
    /// Current position, near a limit. The cockpit's `warning`.
    warn: Color,
    /// Uncertainty, blocked effects, a digest that no longer matches.
    bad: Color,
}

impl Paint {
    fn new(dark: bool, scale: f32) -> Self {
        Self {
            t: tok::theme(dark),
            hair: 1.0 / scale,
            border: tok::theme(dark).indicator,
            dim: tok::theme(dark).text_secondary,
            ok: tok::accent(4, dark),
            info: tok::accent(0, dark),
            warn: tok::accent(3, dark),
            bad: tok::accent(2, dark),
        }
    }
}

/// The cockpit pads keys to ten columns; at 12 pt mono that is this width.
const KEY_W: f32 = 84.0;

/// The cockpit's `bar(theme, frac, 18)`: eighteen cells.
const BAR_CELLS: usize = 18;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dot {
    Done,
    Current,
    Pending,
    Blocked,
}

pub fn view(s: &Ui, cx: &mut Cx<Ui>) -> El {
    let dark = s.appearance.dark(cx.system_dark());
    let p = Paint::new(dark, cx.env().scale);

    col()
        .key("window")
        .role("window")
        .w_full()
        .h_full()
        .fill(p.t.bg)
        .child(heading(p, s))
        .child(toolbar(p, s))
        .child(
            col()
                .key("content")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .px(space::S3)
                .pb(space::S3)
                .gap(space::S2)
                .children(panels(p, s)),
        )
        .child(status_bar(p, s))
}

fn panels(p: Paint, s: &Ui) -> Vec<El> {
    let card = s.ledger.cards.get(s.selected);

    // Top band: two equal panels side by side, stacking when narrow — the
    // cockpit's `width >= 84` branch, done with a wrapping row instead.
    let band = row()
        .key("top-band")
        .w_full()
        .flex_wrap()
        .gap(space::S2)
        .child(overview_panel(p, s).grow(1.0).min_w(320.0))
        .child(mission_panel(p, s, card).grow(1.0).min_w(320.0));

    let mut out = vec![band];
    match card {
        Some(card) => {
            out.push(pipeline_panel(p, card));
            out.push(limits_panel(p, card));
            out.push(workspace_panel(p, card));
            out.push(commands_panel(p, card));
            out.push(attention_panel(p, card));
            out.push(cli_echo_panel(p, s, card));
            out.push(last_line(p, card));
        }
        None => out.push(last_line_empty(p)),
    }
    out
}

// ---------------------------------------------------------------- chrome

/// `SWITCHYARD COCKPIT ............ updated HH:MM:SS  ● live`
fn heading(p: Paint, s: &Ui) -> El {
    let updated = s
        .ledger
        .read_at_utc
        .clone()
        .unwrap_or_else(|| "--:--:--".to_string());
    let (live, live_color) = if s.auto_refresh {
        ("live", p.ok)
    } else {
        ("paused", p.dim)
    };

    row()
        .key("header")
        .w_full()
        .px(space::S3)
        .pt(space::S2)
        .center_y()
        .justify_between()
        .child(
            text("SWITCHYARD COCKPIT")
                .key("title")
                .font(type_::TITLE.mono())
                .color(p.info),
        )
        .child(
            row()
                .key("header-status")
                .gap(space::S2)
                .center_y()
                .child(
                    text(format!("updated {updated}"))
                        .key("updated")
                        .font(type_::MONO_CAPTION.mono())
                        .color(p.dim),
                )
                .child(
                    el().key("live-dot")
                        .size(7.0, 7.0)
                        .radius(3.5)
                        .fill(live_color)
                        .motion(motion::SNAPPY),
                )
                .child(
                    text(live)
                        .key("live-label")
                        .font(type_::MONO_CAPTION.mono())
                        .color(p.dim),
                )
                .child(badge(p, "readonly-badge", "READ-ONLY", p.ok)),
        )
}

fn toolbar(p: Paint, s: &Ui) -> El {
    let count = s.ledger.cards.len();
    let position = if count == 0 {
        "0/0".to_string()
    } else {
        format!("{}/{}", s.selected + 1, count)
    };

    row()
        .key("toolbar")
        .w_full()
        .px(space::S3)
        .pb(space::S2)
        .gap(space::S2)
        .center_y()
        .child(button(p, "refresh", "refresh", refresh_now))
        .child(button(
            p,
            "appearance",
            &format!("theme:{}", s.appearance.label().to_lowercase()),
            toggle_appearance,
        ))
        .child(button(
            p,
            "auto",
            if s.auto_refresh {
                "auto:on"
            } else {
                "auto:off"
            },
            toggle_auto,
        ))
        .child(el().key("toolbar-spacer").grow(1.0).h(1.0))
        .child(
            text(format!("mission {position}"))
                .key("mission-position")
                .font(type_::MONO_CAPTION.mono())
                .color(p.dim),
        )
        .child(button(p, "prev", "<", select_prev))
        .child(button(p, "next", ">", select_next))
}

fn button(p: Paint, key: &str, label: &str, action: fn(&mut Ui)) -> El {
    el().key(key)
        .role("button")
        .label(label)
        .focusable(true)
        .h(20.0)
        .px(space::S2)
        .center()
        .border(p.hair, p.border)
        .hover(p.t.hover)
        .press(p.t.press)
        .motion(motion::SNAPPY)
        .on_press(action)
        .child(
            text(label)
                .font(type_::MONO_CAPTION.mono())
                .color(p.dim)
                .a11y_hidden(),
        )
}

fn badge(p: Paint, key: &str, label: &str, color: Color) -> El {
    el().key(key)
        .px(space::S1)
        .center()
        .border(p.hair, color)
        .child(
            text(label)
                .font(type_::MONO_CAPTION.mono())
                .color(color)
                .a11y_hidden(),
        )
}

/// The bottom status bar: the cockpit's footer, carrying where the truth came
/// from and how fresh it is.
fn status_bar(p: Paint, s: &Ui) -> El {
    row()
        .key("status-bar")
        .w_full()
        .h(24.0)
        .px(space::S3)
        .center_y()
        .justify_between()
        .border(p.hair, p.border)
        .child(
            text(format!("db {}", s.ledger.source))
                .key("status-source")
                .font(type_::MONO_CAPTION.mono())
                .color(p.dim)
                .grow(1.0)
                .min_w(0.0)
                .truncate(),
        )
        .child(
            text(format!(
                "{} missions · read #{}",
                s.ledger.cards.len(),
                s.refreshes
            ))
            .key("status-count")
            .font(type_::MONO_CAPTION.mono())
            .color(p.dim),
        )
}

// ---------------------------------------------------------------- panels

/// A numbered, titled, sharp-cornered box with a right-aligned tag, drawn the
/// way the cockpit draws `┌─┐ │ └─┘` panels.
fn panel(p: Paint, num: usize, key: &str, title: &str, tag: Option<El>, body: Vec<El>) -> El {
    let mut head = row()
        .key(format!("{key}-headrow"))
        .w_full()
        .center_y()
        .justify_between()
        .child(
            text(format!("{num}. {}", title.to_uppercase()))
                .key(format!("{key}-title"))
                .font(type_::SECTION.mono())
                .color(p.info),
        );
    if let Some(tag) = tag {
        head = head.child(tag);
    }

    let mut box_ = col()
        .key(key)
        .w_full()
        .padding(space::S2)
        .gap(space::S1)
        .border(p.hair, p.border)
        .child(head);
    for b in body {
        box_ = box_.child(b);
    }
    box_
}

fn tag(key: &str, label: &str, color: Color) -> El {
    text(label)
        .key(key)
        .font(type_::MONO_CAPTION.mono())
        .color(color)
}

/// `key : value` — dim padded key, dim colon, status-coloured mono value.
fn kv(p: Paint, key: &str, k: &str, value: &str, color: Color) -> El {
    row()
        .key(key)
        .w_full()
        .center_y()
        .child(
            text(k.to_string())
                .key(format!("{key}-k"))
                .font(type_::MONO_CAPTION.mono())
                .color(p.dim)
                .w(KEY_W)
                .shrink()
                .truncate(),
        )
        .child(text(": ").font(type_::MONO_CAPTION.mono()).color(p.dim))
        .child(
            text(value.to_string())
                .key(format!("{key}-v"))
                .font(type_::MONO_CAPTION.mono())
                .color(color)
                .grow(1.0)
                .min_w(0.0)
                .truncate(),
        )
}

fn kv_opt(p: Paint, key: &str, k: &str, value: Option<&str>, color: Color) -> El {
    match value {
        Some(v) if !v.is_empty() => kv(p, key, k, v, color),
        _ => kv(p, key, k, "\u{2014}", p.dim),
    }
}

/// The cockpit's `▕████░░░░▏`: eighteen cells, filled in the status colour.
fn bar(p: Paint, key: &str, done: usize, total: usize, color: Color) -> El {
    let filled = (done * BAR_CELLS + total / 2)
        .checked_div(total)
        .unwrap_or(0)
        .min(BAR_CELLS);
    let mut r = row().key(key).w_full().gap(1.0).h(8.0);
    for i in 0..BAR_CELLS {
        r = r.child(
            el().key(format!("{key}-c{i}"))
                .grow(1.0)
                .h_full()
                .fill(if i < filled { color } else { p.t.control_track }),
        );
    }
    r
}

// ---------------------------------------------------------------- band

fn overview_panel(p: Paint, s: &Ui) -> El {
    let readable = s.ledger.error.is_none();
    let mut rows = vec![
        kv(p, "ov-db", "database", &s.ledger.source, p.t.text),
        kv(
            p,
            "ov-missions",
            "missions",
            &s.ledger.cards.len().to_string(),
            p.t.text,
        ),
        kv(p, "ov-lock", "owner lock", "not taken (read-only)", p.ok),
    ];
    if let Some(error) = &s.ledger.error {
        rows.push(kv(p, "ledger-error", "read error", error, p.bad));
    }
    let ov_tag = if readable {
        tag("ov-tag", "ledger \u{2713}", p.ok)
    } else {
        tag("ov-tag", "ledger !", p.bad)
    };
    panel(p, 1, "overview", "Overview", Some(ov_tag), rows)
}

fn mission_panel(p: Paint, s: &Ui, card: Option<&MissionCard>) -> El {
    let Some(card) = card else {
        let (label, detail, color) = match &s.ledger.error {
            Some(error) => ("ledger unreadable", error.clone(), p.bad),
            None => (
                "no missions",
                "switchyard mission create starts one".to_string(),
                p.dim,
            ),
        };
        return panel(
            p,
            2,
            "mission",
            "Mission",
            None,
            vec![
                kv(p, "no-mission-label", "status", label, color),
                kv(p, "no-mission-detail", "detail", &detail, p.dim),
            ],
        );
    };

    let phase_color = if card.needs_attention() {
        p.bad
    } else if card.phase.is_terminal() {
        p.ok
    } else {
        p.info
    };
    let pr_tag = match card.delivery_state.as_deref() {
        Some(state) => tag("mission-tag", state, p.ok),
        None => tag("mission-tag", "no pr", p.dim),
    };

    panel(
        p,
        2,
        "mission",
        "Mission",
        Some(pr_tag),
        vec![
            kv(p, "mission-id", "mission", &card.mission_id, p.info),
            kv(
                p,
                "mission-phase",
                "phase",
                card.phase.as_str(),
                phase_color,
            ),
            kv(
                p,
                "revision",
                "revision",
                &card.revision.to_string(),
                p.t.text,
            ),
            kv(p, "objective", "objective", &card.objective, p.t.text),
            kv(p, "attempt", "attempt", &card.attempt_id, p.dim),
            kv(p, "runtime", "runner", &card.runtime_adapter, p.dim),
            kv(p, "base-sha", "base", &card.base_sha, p.dim),
            kv(p, "head-sha", "head", &card.head_sha, p.dim),
        ],
    )
}

// ---------------------------------------------------------------- pipeline

fn pipeline_panel(p: Paint, card: &MissionCard) -> El {
    let progress = card.progress();
    let blocked = card.execution_blocked;
    let (done, total) = progress.unwrap_or((0, HAPPY_PATH.len()));

    let meter_color = if blocked { p.bad } else { p.ok };
    let mut meter_row = row()
        .key("pipeline-meter-row")
        .w_full()
        .gap(space::S2)
        .center_y()
        .child(
            // The bar is `.w_full()` for the column-mounted limit meters; here
            // it lives in a row, so it grows inside a bounded wrapper instead
            // of pushing the count and tail off the edge.
            el().key("pipeline-meter-wrap")
                .grow(1.0)
                .min_w(0.0)
                .h(8.0)
                .child(bar(p, "pipeline-meter", done, total, meter_color)),
        )
        .child(
            text(format!("{done}/{total}"))
                .key("pipeline-count")
                .font(type_::MONO_CAPTION.mono())
                .color(p.t.text),
        );

    // The tail: `· ◉ CURRENT`, `· ✦ ALL CLEAR`, or the off-path admission.
    let (tail, tail_color) = match progress {
        None => (
            format!("! {} \u{00b7} off the forward path", card.phase.as_str()),
            p.warn,
        ),
        Some(_) if blocked => (format!("! {} \u{00b7} blocked", card.phase.as_str()), p.bad),
        Some((current, total)) if current + 1 == total => {
            let _ = (current, total);
            ("\u{2726} ALL CLEAR".to_string(), p.ok)
        }
        Some(_) => (
            format!("\u{25c9} {}", card.phase.as_str().to_uppercase()),
            p.warn,
        ),
    };
    meter_row = meter_row.child(
        text(tail)
            .key("pipeline-summary")
            .font(type_::MONO_CAPTION.mono())
            .color(tail_color)
            .grow(1.0)
            .min_w(0.0)
            .truncate(),
    );

    panel(
        p,
        3,
        "pipeline",
        "Pipeline",
        Some(tag("pipeline-tag", &format!("{done}/{total}"), p.dim)),
        vec![meter_row, phase_grid(p, card)],
    )
}

/// Named status dots in a wrapping grid: the cockpit's `phaseGrid`.
fn phase_grid(p: Paint, card: &MissionCard) -> El {
    let progress = card.progress();
    let blocked = card.execution_blocked;

    let mut grid = row()
        .key("phase-grid")
        .w_full()
        .flex_wrap()
        .gap(space::S1)
        .center_y();

    for (i, phase) in HAPPY_PATH.iter().enumerate() {
        let state = match progress {
            None => Dot::Blocked,
            Some(_) if blocked => Dot::Blocked,
            Some((current, _)) if i < current => Dot::Done,
            Some((current, _)) if i == current => Dot::Current,
            Some(_) => Dot::Pending,
        };
        let (dot_color, name_color) = match state {
            Dot::Done => (p.ok, p.t.text),
            Dot::Current => (p.warn, p.info),
            Dot::Pending => (p.t.control_off, p.dim),
            Dot::Blocked => (p.bad, p.bad),
        };

        let mut dot = el()
            .key(format!("dot-{i}"))
            .size(8.0, 8.0)
            .radius(4.0)
            .fill(dot_color)
            .motion(motion::SNAPPY);
        if state == Dot::Current {
            dot = dot.border(p.hair * 2.0, p.t.text);
        }

        grid = grid.child(
            row()
                .key(format!("cell-{i}"))
                .gap(4.0)
                .center_y()
                .min_w(150.0)
                .grow(1.0)
                .child(dot)
                .child(
                    text(phase.as_str())
                        .key(format!("dot-{i}-name"))
                        .font(type_::MONO_CAPTION.mono())
                        .color(name_color),
                ),
        );
    }
    grid
}

// ---------------------------------------------------------------- rest

fn limits_panel(p: Paint, card: &MissionCard) -> El {
    let ratio_color = |used: u32, max: u32| {
        if max == 0 {
            p.dim
        } else if used >= max {
            p.bad
        } else if used * 2 >= max {
            p.warn
        } else {
            p.ok
        }
    };

    let review = match card.review_matches() {
        Some(true) => ("matches content", p.ok),
        Some(false) => ("DOES NOT MATCH content", p.bad),
        None => ("nothing reviewed yet", p.dim),
    };

    panel(
        p,
        4,
        "limits",
        "Limits & Digests",
        None,
        vec![
            kv(
                p,
                "repairs",
                "repairs",
                &format!("{} / {}", card.repair_count, card.max_repairs),
                ratio_color(card.repair_count, card.max_repairs),
            ),
            bar(
                p,
                "repairs-meter",
                card.repair_count as usize,
                card.max_repairs as usize,
                ratio_color(card.repair_count, card.max_repairs),
            ),
            kv(
                p,
                "plan-revisions",
                "plan revs",
                &format!("{} / {}", card.plan_revision_count, card.max_plan_revisions),
                ratio_color(card.plan_revision_count, card.max_plan_revisions),
            ),
            bar(
                p,
                "plan-revisions-meter",
                card.plan_revision_count as usize,
                card.max_plan_revisions as usize,
                ratio_color(card.plan_revision_count, card.max_plan_revisions),
            ),
            kv(
                p,
                "content-digest",
                "content",
                &card.content_digest,
                p.t.text,
            ),
            kv_opt(
                p,
                "plan-digest",
                "plan",
                card.plan_digest.as_deref(),
                p.t.text,
            ),
            kv_opt(
                p,
                "reviewed-digest",
                "approved",
                card.reviewed_digest.as_deref(),
                p.t.text,
            ),
            kv(p, "review-match", "review", review.0, review.1),
        ],
    )
}

fn workspace_panel(p: Paint, card: &MissionCard) -> El {
    let ws_color = match card.workspace_state.as_deref() {
        Some("ready") | Some("released") => p.ok,
        Some("uncertain") => p.bad,
        Some(_) => p.warn,
        None => p.dim,
    };
    panel(
        p,
        5,
        "workspace",
        "Workspace & Delivery",
        None,
        vec![
            kv_opt(
                p,
                "workspace-state",
                "worktree",
                card.workspace_state.as_deref(),
                ws_color,
            ),
            kv_opt(
                p,
                "workspace-path",
                "path",
                card.workspace_path.as_deref(),
                p.dim,
            ),
            kv_opt(
                p,
                "delivery-state",
                "publication",
                card.delivery_state.as_deref(),
                p.ok,
            ),
            kv_opt(
                p,
                "target-branch",
                "branch",
                card.target_branch.as_deref(),
                p.t.text,
            ),
        ],
    )
}

fn commands_panel(p: Paint, card: &MissionCard) -> El {
    let mut rows = vec![kv(
        p,
        "commands-pending",
        "pending",
        &card.pending_commands.len().to_string(),
        if card.pending_commands.is_empty() {
            p.dim
        } else {
            p.warn
        },
    )];

    for (i, command) in card.pending_commands.iter().take(4).enumerate() {
        rows.push(command_row(p, &format!("pending-{i}"), command, p.warn));
    }
    for (i, command) in card.commands.iter().take(8).enumerate() {
        rows.push(command_row(p, &format!("command-{i}"), command, p.t.text));
    }
    if card.pending_commands.is_empty() && card.commands.is_empty() {
        rows.push(kv(
            p,
            "commands-none",
            "history",
            "no commands recorded",
            p.dim,
        ));
    }

    panel(p, 6, "commands", "Commands", None, rows)
}

fn command_row(p: Paint, key: &str, command: &crate::ledger::CommandRow, color: Color) -> El {
    row()
        .key(key)
        .w_full()
        .center_y()
        .child(
            text(command.kind.clone())
                .key(format!("{key}-kind"))
                .font(type_::MONO_CAPTION.mono())
                .color(color)
                .w(220.0)
                .shrink()
                .truncate(),
        )
        .child(text("  ").font(type_::MONO_CAPTION.mono()))
        .child(
            text(command.state.clone())
                .key(format!("{key}-state"))
                .font(type_::MONO_CAPTION.mono())
                .color(p.dim)
                .w(100.0)
                .shrink()
                .truncate(),
        )
        .child(
            text(command.origin.clone())
                .key(format!("{key}-origin"))
                .font(type_::MONO_CAPTION.mono())
                .color(p.dim)
                .grow(1.0)
                .min_w(0.0)
                .truncate(),
        )
}

fn attention_panel(p: Paint, card: &MissionCard) -> El {
    if !card.needs_attention() && card.quarantined == 0 && !card.reconcile_only {
        return panel(
            p,
            7,
            "attention",
            "Attention",
            None,
            vec![kv(
                p,
                "attention-clear",
                "status",
                "clear \u{00b7} nothing needs an operator",
                p.ok,
            )],
        );
    }

    let mut rows = Vec::new();
    if let Some(reason) = &card.attention_reason {
        rows.push(kv(p, "attention-reason", "reason", reason, p.bad));
    }
    if card.execution_blocked {
        rows.push(kv(
            p,
            "attention-blocked",
            "execution",
            "blocked \u{00b7} an effect is uncertain and was not repeated",
            p.bad,
        ));
    }
    if card.reconcile_only {
        rows.push(kv(
            p,
            "attention-reconcile",
            "recovery",
            "reconcile only",
            p.warn,
        ));
    }
    if card.quarantined > 0 {
        rows.push(kv(
            p,
            "attention-quarantined",
            "quarantined",
            &card.quarantined.to_string(),
            p.warn,
        ));
    }
    if rows.is_empty() {
        rows.push(kv(p, "attention-clear", "status", "clear", p.ok));
    }

    panel(p, 7, "attention", "Attention", None, rows)
}

/// Panel 8: the cockpit's "insert, not execute" move. Every invocation is
/// selectable text a person can copy; the dashboard itself runs nothing, and
/// anything that would mutate is badged MUTATES with the caveat that makes it
/// safe to run by hand.
fn cli_echo_panel(p: Paint, s: &Ui, card: &MissionCard) -> El {
    let echoes = s.ledger.cli_echo(card);
    let rows: Vec<El> = echoes
        .iter()
        .enumerate()
        .map(|(i, e)| echo_row(p, i, e))
        .collect();
    panel(
        p,
        8,
        "cli-echo",
        "CLI Echo \u{00b7} Insert, Not Execute",
        None,
        rows,
    )
}

fn echo_row(p: Paint, i: usize, echo: &CliEcho) -> El {
    let key = format!("cli-{i}");
    let (flag, flag_color) = if echo.mutates {
        ("MUTATES", p.warn)
    } else {
        ("READ-ONLY", p.ok)
    };

    let mut block = col().key(key.clone()).w_full().child(
        row()
            .key(format!("{key}-head"))
            .gap(space::S2)
            .center_y()
            .child(badge(p, &format!("{key}-flag"), flag, flag_color))
            .child(
                text(echo.label.clone())
                    .key(format!("{key}-label"))
                    .font(type_::MONO_CAPTION.mono())
                    .color(p.t.text),
            ),
    );

    block = block.child(
        text(echo.command.clone())
            .key(format!("{key}-cmd"))
            .font(type_::MONO_CAPTION.mono())
            .color(p.t.text)
            .w_full()
            .wrap_anywhere()
            .selectable(true)
            .selection_color(p.info),
    );

    if let Some(note) = &echo.note {
        block = block.child(
            text(note.clone())
                .key(format!("{key}-note"))
                .font(type_::MONO_CAPTION.mono())
                .color(p.dim)
                .w_full()
                .wrap(),
        );
    }

    block
}

/// The cockpit's slim `last  …` line under the panels.
fn last_line(p: Paint, card: &MissionCard) -> El {
    let latest = card.pending_commands.first().or(card.commands.first());
    let Some(command) = latest else {
        return last_line_empty(p);
    };
    let color = match command.state.as_str() {
        "committed" => p.ok,
        "pending" => p.warn,
        _ => p.dim,
    };
    row()
        .key("last-line")
        .w_full()
        .px(space::S1)
        .center_y()
        .child(text("last  ").font(type_::MONO_CAPTION.mono()).color(p.dim))
        .child(
            text(format!(
                "{} {} ({})",
                command.kind, command.state, command.origin
            ))
            .key("last-line-v")
            .font(type_::MONO_CAPTION.mono())
            .color(color)
            .grow(1.0)
            .min_w(0.0)
            .truncate(),
        )
}

fn last_line_empty(p: Paint) -> El {
    row()
        .key("last-line")
        .w_full()
        .px(space::S1)
        .center_y()
        .child(text("last  ").font(type_::MONO_CAPTION.mono()).color(p.dim))
        .child(
            text("no commands recorded")
                .key("last-line-v")
                .font(type_::MONO_CAPTION.mono())
                .color(p.dim),
        )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use crate::ledger::Ledger;

    fn render(ledger: Ledger) -> rune::Runtime<Ui> {
        let state = Ui {
            ledger,
            auto_refresh: false,
            ..Ui::default()
        };
        let mut rt = rune::Runtime::new(state, view, (1080.0, 780.0), rune::Env::new(true, 2.0));
        rt.frame(0.0);
        rt
    }

    /// The text an element shows, looked up by key.
    fn text_of(rt: &rune::Runtime<Ui>, key: &str) -> Option<String> {
        rt.elements()
            .iter()
            .find(|e| e.key.as_deref() == Some(key))
            .and_then(|e| e.text.clone())
    }

    /// The accessible name of a control, looked up by key.
    fn label_of(rt: &rune::Runtime<Ui>, key: &str) -> Option<String> {
        rt.elements()
            .iter()
            .find(|e| e.key.as_deref() == Some(key))
            .and_then(|e| e.label.clone())
    }

    fn has(rt: &rune::Runtime<Ui>, key: &str) -> bool {
        rt.elements().iter().any(|e| e.key.as_deref() == Some(key))
    }

    #[test]
    fn the_layout_is_the_cockpits_numbered_panels() {
        let rt = render(Ledger::fixture_planning());
        for key in [
            "overview",
            "mission",
            "pipeline",
            "limits",
            "workspace",
            "commands",
            "attention",
            "cli-echo",
        ] {
            assert!(has(&rt, key), "panel {key} is drawn");
        }
        assert_eq!(
            text_of(&rt, "overview-title").as_deref(),
            Some("1. OVERVIEW")
        );
        assert_eq!(text_of(&rt, "mission-title").as_deref(), Some("2. MISSION"));
        assert_eq!(
            text_of(&rt, "pipeline-title").as_deref(),
            Some("3. PIPELINE")
        );
        assert_eq!(
            text_of(&rt, "attention-title").as_deref(),
            Some("7. ATTENTION")
        );
    }

    #[test]
    fn the_top_band_puts_overview_and_mission_side_by_side() {
        let rt = render(Ledger::fixture_planning());
        // Presented rects are [x, y, w, h].
        let overview = rt
            .presented("overview")
            .map(|(r, _)| r)
            .expect("overview laid out");
        let mission = rt
            .presented("mission")
            .map(|(r, _)| r)
            .expect("mission laid out");
        assert!(
            mission[0] > overview[0] + overview[2] - 1.0,
            "mission starts right of overview: {:?} vs {:?}",
            overview,
            mission
        );
        assert!(
            (overview[1] - mission[1]).abs() < 0.5,
            "the two boxes share a top edge: {:?} vs {:?}",
            overview,
            mission
        );
    }

    #[test]
    fn the_heading_carries_the_updated_time_and_the_live_dot() {
        let rt = render(Ledger::fixture_planning());
        let updated = text_of(&rt, "updated").unwrap_or_default();
        assert!(updated.starts_with("updated "), "got: {updated}");
        assert!(has(&rt, "live-dot"));
        assert!(has(&rt, "readonly-badge"));
    }

    #[test]
    fn kv_rows_keep_the_dim_key_and_colon_shape() {
        let rt = render(Ledger::fixture_planning());
        assert_eq!(text_of(&rt, "mission-phase-k").as_deref(), Some("phase"));
        assert_eq!(text_of(&rt, "mission-phase-v").as_deref(), Some("planning"));
        assert_eq!(text_of(&rt, "revision-v").as_deref(), Some("4"));
    }

    #[test]
    fn the_pipeline_draws_one_named_dot_per_forward_phase() {
        let rt = render(Ledger::fixture_planning());
        let dots = (0..HAPPY_PATH.len())
            .filter(|i| has(&rt, &format!("dot-{i}")))
            .count();
        assert_eq!(dots, HAPPY_PATH.len());
        let names = (0..HAPPY_PATH.len())
            .filter(|i| has(&rt, &format!("dot-{i}-name")))
            .count();
        assert_eq!(names, HAPPY_PATH.len(), "every dot carries its phase name");
        assert_eq!(text_of(&rt, "dot-1-name").as_deref(), Some("planning"));
    }

    #[test]
    fn the_meter_is_eighteen_cells() {
        let rt = render(Ledger::fixture_planning());
        let cells = (0..BAR_CELLS)
            .filter(|i| has(&rt, &format!("pipeline-meter-c{i}")))
            .count();
        assert_eq!(cells, BAR_CELLS);
    }

    #[test]
    fn the_meter_row_keeps_its_count_and_tail_inside_the_window() {
        let rt = render(Ledger::fixture_awaiting_review());
        let count = rt
            .presented("pipeline-count")
            .map(|(r, _)| r)
            .expect("the count is laid out");
        assert!(
            count[0] + count[2] <= 1080.0,
            "count stays inside the window: {count:?}"
        );
        let summary = text_of(&rt, "pipeline-summary").unwrap_or_default();
        assert!(!summary.is_empty(), "the current-phase tail is drawn");
    }

    #[test]
    fn an_off_path_phase_reports_itself_instead_of_faking_progress() {
        let rt = render(Ledger::fixture_needs_attention());
        let summary = text_of(&rt, "pipeline-summary").unwrap_or_default();
        assert!(summary.contains("off the forward path"), "got: {summary}");
    }

    #[test]
    fn uncertainty_is_surfaced_not_smoothed_over() {
        let rt = render(Ledger::fixture_needs_attention());
        let reason = text_of(&rt, "attention-reason-v").unwrap_or_default();
        assert!(
            reason.contains("uncertain external effect"),
            "got: {reason}"
        );
        let blocked = text_of(&rt, "attention-blocked-v").unwrap_or_default();
        assert!(blocked.contains("was not repeated"), "got: {blocked}");
    }

    #[test]
    fn a_clear_mission_says_so_calmly() {
        let rt = render(Ledger::fixture_planning());
        let status = text_of(&rt, "attention-clear-v").unwrap_or_default();
        assert!(status.starts_with("clear"), "got: {status}");
    }

    #[test]
    fn a_digest_that_drifted_from_the_approval_is_called_out() {
        let rt = render(Ledger::fixture_needs_attention());
        assert_eq!(
            text_of(&rt, "review-match-v").as_deref(),
            Some("DOES NOT MATCH content")
        );

        let rt = render(Ledger::fixture_awaiting_review());
        assert_eq!(
            text_of(&rt, "review-match-v").as_deref(),
            Some("matches content")
        );
    }

    #[test]
    fn the_last_line_names_the_newest_command() {
        let rt = render(Ledger::fixture_awaiting_review());
        let last = text_of(&rt, "last-line-v").unwrap_or_default();
        assert!(last.contains("approve_review"), "got: {last}");
        assert!(last.contains("pending"), "got: {last}");
    }

    #[test]
    fn an_unreadable_ledger_is_reported_in_the_ui() {
        let rt = render(Ledger::fixture_unreadable());
        assert_eq!(text_of(&rt, "ledger-error-v").as_deref(), Some("not found"));
        assert_eq!(
            text_of(&rt, "no-mission-label-v").as_deref(),
            Some("ledger unreadable")
        );
    }

    #[test]
    fn an_empty_ledger_points_at_the_command_that_creates_one() {
        let rt = render(Ledger::fixture_empty());
        let detail = text_of(&rt, "no-mission-detail-v").unwrap_or_default();
        assert!(
            detail.contains("switchyard mission create"),
            "got: {detail}"
        );
    }

    #[test]
    fn the_cli_echo_shows_the_read_and_flags_the_mutations() {
        let rt = render(Ledger::fixture_awaiting_review());
        assert!(has(&rt, "cli-echo"), "panel 8 is drawn for a mission");

        let status = text_of(&rt, "cli-0-cmd").unwrap_or_default();
        assert!(
            status.starts_with("switchyard mission status --db "),
            "got: {status}"
        );
        assert!(status.contains("--mission msn-7f3a1c"), "got: {status}");

        let admit = text_of(&rt, "cli-2-cmd").unwrap_or_default();
        assert!(admit.contains("--kind approve_review"), "got: {admit}");
        assert!(admit.contains("--expected-revision 11"), "got: {admit}");
        assert!(
            admit.contains("--command-id <id-you-generate>"),
            "got: {admit}"
        );
        let note = text_of(&rt, "cli-2-note").unwrap_or_default();
        assert!(
            note.contains("reuse it with the same request"),
            "got: {note}"
        );

        let drawn: Vec<String> = rt
            .elements()
            .iter()
            .filter_map(|e| e.text.clone())
            .collect();
        assert!(drawn.iter().any(|t| t == "MUTATES"), "mutations are badged");
    }

    #[test]
    fn the_cli_echo_is_absent_without_a_mission() {
        let rt = render(Ledger::fixture_empty());
        assert!(
            !has(&rt, "cli-echo"),
            "nothing to echo, so nothing is drawn"
        );
    }

    #[test]
    fn the_theme_control_cycles_and_reports_its_state() {
        let mut ui = Ui::default();
        toggle_appearance(&mut ui);
        assert_eq!(ui.appearance.label(), "Dark");
        let mut rt = rune::Runtime::new(ui, view, (1080.0, 780.0), rune::Env::new(true, 2.0));
        rt.frame(0.0);
        let label = label_of(&rt, "appearance").unwrap_or_default();
        assert!(label.contains("dark"), "got: {label}");
        assert_eq!(rt.state.appearance, crate::Appearance::Dark);
    }

    #[test]
    fn the_refresh_button_is_pressable() {
        let mut rt = render(Ledger::fixture_planning());
        let (x, y) = rt.find("refresh").expect("the refresh button is laid out");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..3 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.next_due, 0.0, "a press asks for an immediate read");
    }

    #[test]
    fn selection_moves_within_the_ledger_and_clamps_at_the_ends() {
        let mut ui = Ui {
            ledger: Ledger::fixture_planning(),
            auto_refresh: false,
            ..Ui::default()
        };
        select_next(&mut ui);
        assert_eq!(ui.selected, 0, "one mission: Next does not run off the end");
        select_prev(&mut ui);
        assert_eq!(ui.selected, 0);
    }
}
