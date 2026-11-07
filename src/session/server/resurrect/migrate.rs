//! In-memory metadata migrations. Reading never rewrites the snapshot on disk.
use super::{OLDEST_READABLE_SNAPSHOT_VERSION, SNAPSHOT_VERSION};
use serde_json::Value;

type Migration = fn(&mut Value) -> Result<(), ()>;

/// Each entry upgrades exactly one format, starting at the oldest readable format.
/// A format bump must append its migration before the new writer can ship.
const STEPS: &[Migration] = &[v2_to_v3];
const _: () =
    assert!(STEPS.len() == (SNAPSHOT_VERSION - OLDEST_READABLE_SNAPSHOT_VERSION) as usize);

pub(super) fn to_current(document: &mut Value, version: u32) -> Result<(), ()> {
    for from in version..SNAPSHOT_VERSION {
        let step = STEPS
            .get((from - OLDEST_READABLE_SNAPSHOT_VERSION) as usize)
            .ok_or(())?;
        step(document)?;
        document["version"] = Value::from(from + 1);
    }
    Ok(())
}

fn v2_to_v3(document: &mut Value) -> Result<(), ()> {
    let panes = document
        .get_mut("panes")
        .and_then(Value::as_array_mut)
        .ok_or(())?;
    for pane in panes {
        // v2 captured launch intent, but never observed a running foreground command.
        // Preserve launch verbatim, including direct argv and shell quoting.
        pane.as_object_mut()
            .ok_or(())?
            .insert("foreground".into(), Value::Null);
    }
    Ok(())
}
