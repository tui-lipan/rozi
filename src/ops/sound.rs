use crate::AppRoot;
use crate::platform::sound::Cue;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tui_lipan::prelude::Context;

pub(crate) fn cue(ctx: &mut Context<AppRoot>, cue: Cue) {
    let sounds = &ctx.state.config.sounds;
    if ctx.state.do_not_disturb
        || !sounds.enabled
        || !sounds.enabled_for(cue)
        || !should_play(
            &mut ctx.state.sound_cues,
            cue,
            Instant::now(),
            sounds.throttle_ms,
        )
    {
        return;
    }
    crate::platform::sound::play(
        cue,
        sounds.file_for(cue).map(std::path::PathBuf::as_path),
        sounds.player.as_deref(),
    );
}

pub(crate) fn should_play(
    map: &mut HashMap<Cue, Instant>,
    cue: Cue,
    now: Instant,
    throttle_ms: u64,
) -> bool {
    if map
        .get(&cue)
        .is_some_and(|last| now.duration_since(*last) < Duration::from_millis(throttle_ms))
    {
        return false;
    }
    map.insert(cue, now);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn throttle_is_per_cue() {
        let mut cues = HashMap::new();
        let now = Instant::now();
        assert!(should_play(&mut cues, Cue::Bell, now, 2000));
        assert!(!should_play(&mut cues, Cue::Bell, now, 2000));
        assert!(should_play(&mut cues, Cue::Done, now, 2000));
    }
}
