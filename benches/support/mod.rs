#![allow(dead_code)]

use hyprmux::layout_tree_ser::{SerializedLayoutKind, SerializedSplitAxis, SerializedTree};
use hyprmux::session::protocol::{
    ClientInfo, PROTOCOL_VERSION, PaneCommandPhase, PaneCwdSource, PaneMeta, PaneRuntimeState,
    ServerMessage,
};
use hyprmux::shared_layout::{
    FracRect, SHARED_LAYOUT_VERSION, SharedLayout, SharedPane, SharedWorkspace,
};
use tui_lipan::prelude::TerminalScreen;

const CORPUS_BYTES: usize = 256 * 1024;

pub fn screen(cols: u16, rows: u16) -> TerminalScreen {
    TerminalScreen::new(rows, cols, 5_000)
}

pub fn plain_lines() -> Vec<u8> {
    repeat_to_size(
        b"2026-07-14T12:34:56.789Z INFO worker=17 request=8f31 elapsed_ms=12 path=/api/items\r\n",
        CORPUS_BYTES,
    )
}

pub fn sgr_heavy() -> Vec<u8> {
    repeat_to_size(
        b"\x1b[1;38;5;196mERROR\x1b[0m \x1b[38;2;80;160;240mservice\x1b[0m \x1b[4;3;48;5;235mstyled payload\x1b[0m\r\n",
        CORPUS_BYTES,
    )
}

pub fn scroll_regions() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(CORPUS_BYTES);
    let mut line = 0_u32;
    while bytes.len() < CORPUS_BYTES {
        bytes.extend_from_slice(b"\x1b[2;23r");
        bytes.extend_from_slice(
            format!(
                "\x1b[{};1Hline {line:06} | editor body\x1b[K",
                2 + line % 22
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(b"\x1b[23;1H\n\x1b[1;1Hstatus: NORMAL\x1b[K");
        line += 1;
    }
    bytes.truncate(CORPUS_BYTES);
    bytes
}

pub fn wide_unicode() -> Vec<u8> {
    let line = "東京の端末 測試資料 안녕하세요 🦀🚀🙂 family: 👨‍👩‍👧‍👦 combining: e\u{301}\r\n";
    let mut bytes = Vec::with_capacity(CORPUS_BYTES);
    while bytes.len() + line.len() <= CORPUS_BYTES {
        bytes.extend_from_slice(line.as_bytes());
    }
    bytes
}

pub fn cat_large() -> Vec<u8> {
    let mut line = b"\x1b[0m".to_vec();
    line.extend(std::iter::repeat_n(b'x', 4_089));
    line.extend_from_slice(b"\r\n");
    repeat_to_size(&line, 1024 * 1024)
}

pub fn bytes_of_len(len: usize) -> Vec<u8> {
    let source = plain_lines();
    repeat_to_size(&source, len)
}

pub fn dirty_screen(cols: u16, rows: u16) -> TerminalScreen {
    let mut terminal = screen(cols, rows);
    let mut seed = Vec::with_capacity(usize::from(cols) * usize::from(rows) * 2);
    for row in 0..rows {
        seed.extend_from_slice(
            format!("\x1b[{};1H\x1b[38;5;{}m", row + 1, 16 + row % 216).as_bytes(),
        );
        for col in 0..cols {
            seed.push(b'a' + ((u32::from(row) + u32::from(col)) % 26) as u8);
        }
    }
    terminal.process_bytes(&seed);
    terminal
}

pub fn attached_message() -> ServerMessage {
    let layout = large_layout();
    ServerMessage::Attached {
        created_from_profile: None,
        protocol_version: PROTOCOL_VERSION,
        session: "benchmark-session".to_string(),
        client_id: 7,
        panes: layout
            .workspaces
            .iter()
            .flat_map(|workspace| workspace.panes.iter())
            .map(|pane| PaneMeta {
                pane_id: pane.pane_id,
                generation: pane.generation,
                cols: 200,
                rows: 60,
                pid: Some(10_000 + pane.pane_id),
                title: pane.title.clone(),
                exited: None,
                logging: pane.pane_id.is_multiple_of(3),
                runtime: PaneRuntimeState {
                    cwd: pane.cwd.clone(),
                    cwd_host: None,
                    cwd_source: PaneCwdSource::ShellReport,
                    command_phase: PaneCommandPhase::Executing,
                    foreground_program: Some("benchmark-worker".to_string()),
                    last_exit_status: Some(0),
                    status: None,
                    detected_agent: None,
                    sequence: u64::from(pane.pane_id),
                },
            })
            .collect(),
        layout_rev: 4_096,
        layout: Some(layout),
        controller: Some(7),
        clients: (0_u64..32)
            .map(|id| ClientInfo {
                id,
                label: format!("client-{id:02}-workstation"),
                read_only: id % 5 == 0,
                requesting_control: id % 7 == 0,
            })
            .collect(),
        input_locked: false,
    }
}

pub fn layout_committed_message() -> ServerMessage {
    ServerMessage::LayoutCommitted {
        rev: 4_097,
        author: 7,
        layout: large_layout(),
    }
}

fn large_layout() -> SharedLayout {
    SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        canvas_cols: 320,
        canvas_rows: 90,
        workspaces: (0..10)
            .map(|index| {
                let first_id = (index * 16 + 1) as u32;
                let panes = (0..16)
                    .map(|offset| {
                        let pane_id = first_id + offset;
                        SharedPane {
                            pane_id,
                            generation: 100 + u64::from(pane_id),
                            title: Some(format!("workspace-{index}-pane-{offset}")),
                            profile_name: Some(format!("benchmark-profile-{}", offset % 4)),
                            cwd: Some(format!("/workspace/project-{index}/module-{offset}")),
                            command: Some(format!("benchmark-worker --shard {offset} --verbose")),
                            replay: false,
                            keep_open: offset % 3 == 0,
                            floating: offset == 15,
                            fullscreen: false,
                            rect: (offset == 15).then_some(FracRect {
                                x: 0.15,
                                y: 0.1,
                                w: 0.7,
                                h: 0.75,
                            }),
                        }
                    })
                    .collect();
                SharedWorkspace {
                    index,
                    name: Some(format!("benchmark-workspace-{index}")),
                    synchronized: index % 2 == 0,
                    layout: SerializedLayoutKind::Dwindle,
                    start_axis: SerializedSplitAxis::Horizontal,
                    split_ratios: (0..15)
                        .map(|offset| 0.35 + (offset % 5) as f32 * 0.05)
                        .collect(),
                    tree: Some(balanced_tree(first_id, 16)),
                    panes,
                }
            })
            .collect(),
    }
}

fn balanced_tree(first: u32, count: u32) -> SerializedTree<u32> {
    if count == 1 {
        return SerializedTree::Leaf { pane: first };
    }
    let left = count / 2;
    SerializedTree::Split {
        axis: if count.is_multiple_of(2) {
            SerializedSplitAxis::Horizontal
        } else {
            SerializedSplitAxis::Vertical
        },
        ratio: 0.5,
        first: Box::new(balanced_tree(first, left)),
        second: Box::new(balanced_tree(first + left, count - left)),
    }
}

fn repeat_to_size(pattern: &[u8], size: usize) -> Vec<u8> {
    assert!(!pattern.is_empty());
    let mut bytes = Vec::with_capacity(size);
    while bytes.len() < size {
        let remaining = size - bytes.len();
        bytes.extend_from_slice(&pattern[..remaining.min(pattern.len())]);
    }
    bytes
}
