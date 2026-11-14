//! The mission-control dashboard.
//!
//! The layout mirrors the ADW Cockpit Pi extension
//! (`.pi/extensions/adw-cockpit/index.ts`, `renderDashboard`) line for line,
//! translated from terminal box-drawing to Rune primitives:
//!
//! - a heading line: bold accent name at the left, with the last completed read
//!   and a separate polling/read-state indicator at the right;
//! - a top band of two equal panels side by side — `1. OVERVIEW` and
//!   `2. MISSION` (the cockpit's `2. LATEST RUN`) — stacking when the window is
//!   too narrow, exactly as the cockpit stacks below 84 columns;
//! - `3. ATTENTION`: operator needs before detail, followed by `4. PIPELINE`'s
//!   segmented completion meter with `done/total` and a current-phase tail over
//!   a wrapping grid of named status dots
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
    Unknown,
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
            out.push(attention_panel(p, card));
            out.push(pipeline_panel(p, card));
            out.push(limits_panel(p, card));
            out.push(workspace_panel(p, card));
            out.push(commands_panel(p, card));
            out.push(cli_echo_panel(p, s, card));
            out.push(last_line(p, card));
        }
        None => out.push(no_mission_last_line(p, &s.ledger)),
    }
    out
}

// ---------------------------------------------------------------- chrome

/// `SWITCHYARD COCKPIT ............ updated HH:MM:SS UTC  ● polling`
///
/// Polling, an in-flight read, a failed read, and an initial unread state are
/// different facts. The header names them instead of presenting auto-refresh
/// as proof of liveness.
fn heading(p: Paint, s: &Ui) -> El {
    let updated = s
        .ledger
        .read_at_utc
        .as_ref()
        .map(|time| format!("updated {time} UTC"))
        .unwrap_or_else(|| "no completed read".to_string());
    let (read_state, state_color) = if s.fetch.is_pending() {
        ("reading", p.info)
    } else if s.ledger.error.is_some() {
        ("read failed", p.bad)
    } else if s.ledger.read_at_utc.is_none() {
        ("not read", p.warn)
    } else if s.auto_refresh {
        ("polling", p.ok)
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
                    text(updated)
                        .key("updated")
                        .font(type_::MONO_CAPTION.mono())
                        .color(p.dim),
                )
                .child(
                    el().key("read-state-dot")
                        .size(7.0, 7.0)
                        .radius(3.5)
                        .fill(state_color)
                        .motion(motion::SNAPPY),
                )
                .child(
                    text(read_state)
                        .key("read-state-label")
                        .font(type_::MONO_CAPTION.mono())
                        .color(state_color),
                )
                .child(badge(p, "readonly-badge", "READ-ONLY", p.ok)),
        )
}

fn toolbar(p: Paint, s: &Ui) -> El {
    let count = s.ledger.cards.len();
    let position = if s.ledger.read_at_utc.is_none() || s.ledger.error.is_some() {
        "\u{2014}".to_string()
    } else if count == 0 {
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
        .child(button_with_state(
            p,
            "prev",
            "\u{2039}",
            "previous mission",
            select_prev,
            s.selected > 0,
        ))
        .child(button_with_state(
            p,
            "next",
            "\u{203a}",
            "next mission",
            select_next,
            s.selected + 1 < count,
        ))
}

fn button(p: Paint, key: &str, label: &str, action: fn(&mut Ui)) -> El {
    button_with_state(p, key, label, label, action, true)
}

fn button_with_state(
    p: Paint,
    key: &str,
    caption: &str,
    accessible_label: &str,
    action: fn(&mut Ui),
    enabled: bool,
) -> El {
    let accessible_label = if enabled {
        accessible_label.to_string()
    } else {
        format!("{accessible_label} (unavailable)")
    };
    let mut control = el()
        .key(key)
        .role("button")
        .label(&accessible_label)
        .focusable(enabled)
        .h(24.0)
        .px(space::S2)
        .center()
        .border(p.hair, if enabled { p.border } else { p.t.control_track })
        .motion(motion::SNAPPY);
    if enabled {
        control = control.hover(p.t.hover).press(p.t.press).on_press(action);
    }
    control.child(
        text(caption)
            .font(type_::MONO_CAPTION.mono())
            .color(if enabled { p.dim } else { p.t.control_off })
            .a11y_hidden(),
    )
}

fn badge(p: Paint, key: &str, label: &str, color: Color) -> El {
    el().key(key)
        .label(label)
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
    let missions = if s.ledger.read_at_utc.is_none() || s.ledger.error.is_some() {
        "missions —".to_string()
    } else {
        let count = s.ledger.cards.len();
        format!("{count} mission{}", if count == 1 { "" } else { "s" })
    };
    let read = if s.ledger.error.is_some() {
        format!("read failed · #{}", s.refreshes)
    } else {
        format!("read #{}", s.refreshes)
    };
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
            text(format!("{missions} · {read}"))
                .key("status-count")
                .font(type_::MONO_CAPTION.mono())
                .color(if s.ledger.error.is_some() {
                    p.bad
                } else {
                    p.dim
                }),
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

/// A `key : value` row for prose that must remain readable rather than being
/// silently clipped. IDs, paths, and digests stay in the compact `kv` form.
fn kv_wrapped(p: Paint, key: &str, k: &str, value: &str, color: Color) -> El {
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
                .wrap(),
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

/// Keep the aggregate count and the selected mission's panel on the same rule.
fn has_operator_attention(card: &MissionCard) -> bool {
    card.needs_attention() || card.quarantined > 0 || card.reconcile_only
}

fn overview_panel(p: Paint, s: &Ui) -> El {
    let data_known = s.ledger.read_at_utc.is_some() && s.ledger.error.is_none();
    let attention = s
        .ledger
        .cards
        .iter()
        .filter(|card| has_operator_attention(card))
        .count();
    let mission_count = if data_known {
        s.ledger.cards.len().to_string()
    } else {
        "\u{2014}".to_string()
    };
    let attention_label = if !data_known {
        "\u{2014}".to_string()
    } else if attention == 0 {
        "clear".to_string()
    } else {
        attention.to_string()
    };
    let mut rows = vec![
        kv(p, "ov-db", "database", &s.ledger.source, p.t.text),
        kv(p, "ov-missions", "missions", &mission_count, p.t.text),
        kv(
            p,
            "ov-attention",
            "attention",
            &attention_label,
            if !data_known {
                p.dim
            } else if attention > 0 {
                p.bad
            } else {
                p.ok
            },
        ),
        kv(p, "ov-lock", "owner lock", "not taken (read-only)", p.ok),
    ];
    if let Some(error) = &s.ledger.error {
        rows.push(kv_wrapped(p, "ledger-error", "read error", error, p.bad));
    }
    let ov_tag = if s.ledger.error.is_some() {
        tag("ov-tag", "read failed", p.bad)
    } else if s.ledger.read_at_utc.is_none() {
        tag("ov-tag", "not read", p.warn)
    } else {
        tag("ov-tag", "ledger \u{2713}", p.ok)
    };
    panel(p, 1, "overview", "Overview", Some(ov_tag), rows)
}

fn mission_panel(p: Paint, s: &Ui, card: Option<&MissionCard>) -> El {
    let Some(card) = card else {
        let (label, detail, color) = match &s.ledger.error {
            Some(_) => (
                "ledger unreadable",
                "Correct the database path or permissions, then refresh.".to_string(),
                p.bad,
            ),
            None if s.ledger.read_at_utc.is_none() => (
                "waiting for first read",
                "Use refresh to inspect the ledger; the owner lock is not taken.".to_string(),
                p.warn,
            ),
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
                kv_wrapped(p, "no-mission-detail", "detail", &detail, p.dim),
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
        Some(state @ "published") => tag("mission-tag", state, p.ok),
        Some(state) => tag("mission-tag", state, p.info),
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
            kv_wrapped(p, "objective", "objective", &card.objective, p.t.text),
            kv(
                p,
                "attempt",
                "attempt",
                &format!(
                    "{} \u{00b7} rev {} \u{00b7} {}",
                    card.attempt_id, card.revision, card.runtime_adapter
                ),
                p.dim,
            ),
            kv(
                p,
                "change",
                "base \u{2192} head",
                &format!("{} \u{2192} {}", card.base_sha, card.head_sha),
                p.dim,
            ),
        ],
    )
}

// ---------------------------------------------------------------- pipeline

fn pipeline_panel(p: Paint, card: &MissionCard) -> El {
    let progress = card.progress();
    let blocked = card.execution_blocked;
    let (done, total) = progress.unwrap_or((0, HAPPY_PATH.len()));

    let meter_color = if blocked {
        p.bad
    } else if progress.is_none() {
        p.dim
    } else {
        p.ok
    };
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

    // The tail: `· ◉ CURRENT`, `· ✓ COMPLETE`, or the off-path admission.
    let (tail, tail_color) = match progress {
        None => (
            format!("! {} \u{00b7} off the forward path", card.phase.as_str()),
            p.warn,
        ),
        Some(_) if blocked => (format!("! {} \u{00b7} blocked", card.phase.as_str()), p.bad),
        Some((current, total)) if current + 1 >= total => ("\u{2713} COMPLETE".to_string(), p.ok),
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
        4,
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
            None => Dot::Unknown,
            Some((current, _)) if i < current => Dot::Done,
            Some((current, _)) if blocked && i == current => Dot::Blocked,
            Some((current, _)) if i == current => Dot::Current,
            Some(_) => Dot::Pending,
        };
        let (dot_color, name_color) = match state {
            Dot::Done => (p.ok, p.t.text),
            Dot::Current => (p.warn, p.info),
            Dot::Pending | Dot::Unknown => (p.t.control_off, p.dim),
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
        5,
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
    let publication_color = match card.delivery_state.as_deref() {
        Some("published") => p.ok,
        Some(_) => p.info,
        None => p.dim,
    };
    panel(
        p,
        6,
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
                publication_color,
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
    let hidden_pending = card.pending_commands.len().saturating_sub(4);
    if hidden_pending > 0 {
        rows.push(kv(
            p,
            "pending-more",
            "more",
            &format!("+{hidden_pending} pending not shown"),
            p.warn,
        ));
    }
    let hidden_history = card.commands.len().saturating_sub(8);
    if hidden_history > 0 {
        rows.push(kv(
            p,
            "commands-more",
            "history",
            &format!("+{hidden_history} older not shown"),
            p.dim,
        ));
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

    panel(p, 7, "commands", "Commands", None, rows)
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
    if !has_operator_attention(card) {
        return panel(
            p,
            3,
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
        rows.push(kv_wrapped(p, "attention-reason", "reason", reason, p.bad));
    }
    if card.execution_blocked {
        rows.push(kv_wrapped(
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
        rows.push(kv(
            p,
            "attention-unspecified",
            "status",
            "attention required \u{00b7} no detail recorded",
            p.bad,
        ));
    }

    panel(p, 3, "attention", "Attention", None, rows)
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
    last_line_message(p, "no commands recorded")
}

fn no_mission_last_line(p: Paint, ledger: &crate::ledger::Ledger) -> El {
    if ledger.error.is_some() {
        last_line_message(p, "read failed \u{00b7} mission data unavailable")
    } else if ledger.read_at_utc.is_none() {
        last_line_message(p, "waiting for first ledger read")
    } else {
        last_line_empty(p)
    }
}

fn last_line_message(p: Paint, message: &str) -> El {
    row()
        .key("last-line")
        .w_full()
        .px(space::S1)
        .center_y()
        .child(text("last  ").font(type_::MONO_CAPTION.mono()).color(p.dim))
        .child(
            text(message.to_string())
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

    fn render_at(ledger: Ledger, width: f32, height: f32) -> rune::Runtime<Ui> {
        let state = Ui {
            ledger,
            auto_refresh: false,
            ..Ui::default()
        };
        let mut rt = rune::Runtime::new(state, view, (width, height), rune::Env::new(true, 2.0));
        rt.frame(0.0);
        rt
    }

    fn render(ledger: Ledger) -> rune::Runtime<Ui> {
        render_at(ledger, 1080.0, 780.0)
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
            Some("4. PIPELINE")
        );
        assert_eq!(
            text_of(&rt, "attention-title").as_deref(),
            Some("3. ATTENTION")
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
    fn the_heading_separates_read_time_from_polling_state() {
        let rt = render(Ledger::fixture_planning());
        let updated = text_of(&rt, "updated").unwrap_or_default();
        assert!(updated.starts_with("updated "), "got: {updated}");
        assert!(
            updated.ends_with(" UTC"),
            "the clock is labeled UTC, got: {updated}"
        );
        assert!(has(&rt, "read-state-dot"));
        assert_eq!(text_of(&rt, "read-state-label").as_deref(), Some("paused"));
        assert_eq!(
            label_of(&rt, "readonly-badge").as_deref(),
            Some("READ-ONLY")
        );
    }

    #[test]
    fn the_initial_unread_state_does_not_claim_a_healthy_empty_ledger() {
        let rt = render(Ledger::unread(std::path::Path::new("switchyard.db")));
        assert_eq!(
            text_of(&rt, "updated").as_deref(),
            Some("no completed read")
        );
        assert_eq!(
            text_of(&rt, "read-state-label").as_deref(),
            Some("not read")
        );
        assert_eq!(text_of(&rt, "ov-tag").as_deref(), Some("not read"));
        assert_eq!(
            text_of(&rt, "mission-position").as_deref(),
            Some("mission —")
        );
        assert_eq!(text_of(&rt, "ov-missions-v").as_deref(), Some("—"));
        assert_eq!(text_of(&rt, "ov-attention-v").as_deref(), Some("—"));
        assert_eq!(
            text_of(&rt, "no-mission-label-v").as_deref(),
            Some("waiting for first read")
        );
        assert_eq!(
            text_of(&rt, "status-count").as_deref(),
            Some("missions — · read #0")
        );
        assert_eq!(
            text_of(&rt, "last-line-v").as_deref(),
            Some("waiting for first ledger read")
        );
    }

    #[test]
    fn an_in_flight_read_is_visible_even_when_polling_is_paused() {
        let mut ui = Ui {
            source: std::path::PathBuf::from("/nonexistent/switchyard-ui-reading.db"),
            fetch: rune::Latest::default().debounce(std::time::Duration::from_secs(3600)),
            auto_refresh: false,
            force_refresh: true,
            ..Ui::default()
        };
        let mut tx = rune::Tx::default();
        assert!(crate::poll(&mut ui, 50.0, &mut tx));
        assert!(ui.fetch.is_pending());

        let mut rt = rune::Runtime::new(ui, view, (1080.0, 780.0), rune::Env::new(true, 2.0));
        rt.frame(0.0);
        assert_eq!(text_of(&rt, "read-state-label").as_deref(), Some("reading"));
    }

    #[test]
    fn kv_rows_keep_the_dim_key_and_colon_shape() {
        let rt = render(Ledger::fixture_planning());
        assert_eq!(text_of(&rt, "mission-phase-k").as_deref(), Some("phase"));
        assert_eq!(text_of(&rt, "mission-phase-v").as_deref(), Some("planning"));
        let attempt = text_of(&rt, "attempt-v").unwrap_or_default();
        assert!(attempt.contains("rev 4"), "got: {attempt}");
        assert!(attempt.contains("att-01"), "got: {attempt}");
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
    fn needs_attention_without_detail_never_reads_as_clear() {
        let mut card = MissionCard::blank();
        card.phase = switchyard_core::Phase::NeedsAttention;
        let mut ledger = Ledger::fixture_planning();
        ledger.cards = vec![card];
        let rt = render(ledger);
        assert_eq!(
            text_of(&rt, "attention-unspecified-v").as_deref(),
            Some("attention required · no detail recorded")
        );
        assert!(!has(&rt, "attention-clear"));
    }

    #[test]
    fn quarantine_counts_as_attention_even_without_an_explicit_reason() {
        let mut card = MissionCard::blank();
        card.quarantined = 1;
        let mut ledger = Ledger::fixture_planning();
        ledger.cards = vec![card];
        let rt = render(ledger);
        assert_eq!(text_of(&rt, "ov-attention-v").as_deref(), Some("1"));
        assert_eq!(
            text_of(&rt, "attention-quarantined-v").as_deref(),
            Some("1")
        );
        assert!(!has(&rt, "attention-clear"));
    }

    #[test]
    fn attention_prose_wraps_at_the_minimum_window_width() {
        let short = render_at(Ledger::fixture_needs_attention(), 760.0, 520.0);
        let mut long_ledger = Ledger::fixture_needs_attention();
        long_ledger.cards[0].attention_reason = Some(
            "uncertain external effect; inspect the receipt before any recovery or retry "
                .repeat(8),
        );
        let long = render_at(long_ledger, 760.0, 520.0);
        let short_height = short
            .presented("attention-reason-v")
            .map(|(rect, _)| rect[3])
            .unwrap_or_default();
        let long_height = long
            .presented("attention-reason-v")
            .map(|(rect, _)| rect[3])
            .unwrap_or_default();
        assert!(
            long_height > short_height,
            "the reason wraps to additional lines: {long_height} vs {short_height}"
        );
    }

    #[test]
    fn overview_counts_attention_on_unselected_missions_and_navigation_has_boundaries() {
        let mut ledger = Ledger::fixture_planning();
        ledger
            .cards
            .push(Ledger::fixture_needs_attention().cards.remove(0));
        let mut rt = render(ledger);
        assert_eq!(text_of(&rt, "ov-missions-v").as_deref(), Some("2"));
        assert_eq!(text_of(&rt, "ov-attention-v").as_deref(), Some("1"));
        assert_eq!(
            label_of(&rt, "prev").as_deref(),
            Some("previous mission (unavailable)")
        );
        assert_eq!(label_of(&rt, "next").as_deref(), Some("next mission"));

        let (x, y) = rt.find("next").expect("next is laid out");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..3 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.selected, 1);
        assert_eq!(text_of(&rt, "mission-id-v").as_deref(), Some("msn-2b8e44"));
        assert_eq!(
            label_of(&rt, "next").as_deref(),
            Some("next mission (unavailable)")
        );
    }

    #[test]
    fn capped_command_lists_disclose_the_number_hidden() {
        let mut card = MissionCard::blank();
        card.pending_commands = (0..5)
            .map(|i| crate::ledger::CommandRow {
                kind: format!("pending_{i}"),
                state: "pending".to_string(),
                origin: "operator".to_string(),
            })
            .collect();
        card.commands = (0..10)
            .map(|i| crate::ledger::CommandRow {
                kind: format!("committed_{i}"),
                state: "committed".to_string(),
                origin: "runtime".to_string(),
            })
            .collect();
        let mut ledger = Ledger::fixture_planning();
        ledger.cards = vec![card];
        let rt = render(ledger);
        assert_eq!(
            text_of(&rt, "pending-more-v").as_deref(),
            Some("+1 pending not shown")
        );
        assert_eq!(
            text_of(&rt, "commands-more-v").as_deref(),
            Some("+2 older not shown")
        );
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
    fn with_nothing_pending_the_last_line_is_the_newest_history() {
        let rt = render(Ledger::fixture_planning());
        let last = text_of(&rt, "last-line-v").unwrap_or_default();
        assert!(
            last.starts_with("start_planning"),
            "newest first, not admission order: {last}"
        );
    }

    #[test]
    fn a_published_mission_reads_as_complete_not_one_short() {
        let rt = render(Ledger::fixture_published());
        assert_eq!(text_of(&rt, "pipeline-count").as_deref(), Some("9/9"));
        let summary = text_of(&rt, "pipeline-summary").unwrap_or_default();
        assert!(summary.contains("COMPLETE"), "got: {summary}");
    }

    #[test]
    fn an_unreadable_ledger_is_reported_in_the_ui() {
        let rt = render(Ledger::fixture_unreadable());
        assert_eq!(text_of(&rt, "ledger-error-v").as_deref(), Some("not found"));
        assert_eq!(
            text_of(&rt, "no-mission-label-v").as_deref(),
            Some("ledger unreadable")
        );
        assert_eq!(
            text_of(&rt, "read-state-label").as_deref(),
            Some("read failed")
        );
        assert_eq!(text_of(&rt, "ov-missions-v").as_deref(), Some("—"));
        assert_eq!(
            text_of(&rt, "mission-position").as_deref(),
            Some("mission —")
        );
        assert_eq!(
            text_of(&rt, "no-mission-detail-v").as_deref(),
            Some("Correct the database path or permissions, then refresh.")
        );
        assert_eq!(
            text_of(&rt, "status-count").as_deref(),
            Some("missions — · read failed · #0")
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
        assert_eq!(text_of(&rt, "ov-missions-v").as_deref(), Some("0"));
        assert_eq!(text_of(&rt, "ov-attention-v").as_deref(), Some("clear"));
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
        assert!(status.contains("--mission 'msn-7f3a1c'"), "got: {status}");

        let recover = text_of(&rt, "cli-2-cmd").unwrap_or_default();
        assert!(recover.contains("mission recover --db"), "got: {recover}");
        let note = text_of(&rt, "cli-2-note").unwrap_or_default();
        assert!(note.contains("already admitted"), "got: {note}");
        let drawn_cmds: Vec<String> = rt
            .elements()
            .iter()
            .filter_map(|e| e.text.clone())
            .collect();
        assert!(
            drawn_cmds.iter().all(|t| !t.contains("--rationale ")),
            "no echoed invocation uses a flag the CLI does not have"
        );

        let drawn: Vec<String> = rt
            .elements()
            .iter()
            .filter_map(|e| e.text.clone())
            .collect();
        assert!(drawn.iter().any(|t| t == "MUTATES"), "mutations are badged");
        assert_eq!(label_of(&rt, "cli-0-flag").as_deref(), Some("READ-ONLY"));
        assert_eq!(label_of(&rt, "cli-1-flag").as_deref(), Some("MUTATES"));
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
    fn the_refresh_button_reads_once_while_auto_is_off() {
        let mut rt = render(Ledger::fixture_planning());
        assert!(!rt.state.auto_refresh, "the scene is paused");
        let mut tx = rune::Tx::default();
        crate::poll(&mut rt.state, 50.0, &mut tx);
        assert!(
            !rt.state.fetch.is_pending(),
            "paused and idle does not read"
        );

        let (x, y) = rt.find("refresh").expect("the refresh button is laid out");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..3 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.state.force_refresh, "a press asks for an immediate read");

        crate::poll(&mut rt.state, 50.0, &mut tx);
        assert!(!rt.state.force_refresh, "poll consumes the manual refresh");
        let started = rt.state.fetch.is_pending() || rt.state.fetch.take().is_some();
        assert!(started, "a paused refresh starts one read");
    }

    #[test]
    fn a_paused_dashboard_still_takes_a_manual_refresh_into_poll() {
        let mut ui = Ui {
            ledger: Ledger::fixture_planning(),
            auto_refresh: false,
            ..Ui::default()
        };
        refresh_now(&mut ui);
        assert!(
            ui.read_due(123.0, false),
            "a forced read is due even while paused"
        );
        assert!(
            !ui.read_due(123.0, true),
            "but never on top of an in-flight read"
        );
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
