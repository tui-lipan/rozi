//! Replacing the running client with another build while its sessions carry on.
//!
//! Session servers already outlive the client that started them, and a build attaches to every
//! server whose wire generation it speaks. What a relaunch adds is the client's side of that: the
//! new process comes back to the same sessions, in the same places, holding the same layout
//! control, so that installing an update reads as one frame changing rather than as quitting and
//! starting again.
//!
//! A relaunch is a transaction across two processes:
//!
//! 1. [`candidate`] finds the binary to run and has it describe itself, so nothing about it is
//!    assumed from a path.
//! 2. [`plan`] decides, for each attached session, whether the new build carries it over as it is,
//!    reconnects to it, has to upgrade it, or cannot take it at all.
//! 3. The outgoing client asks each server to hold its slot, records everything in a [`document`],
//!    lets go of its sessions without closing any of them, and gives the terminal back.
//! 4. The runner replaces the process with the candidate (`exec` on Unix, a waited child on
//!    Windows), which upgrades the sessions that need it and then attaches to every session in
//!    turn, claiming the slots that were held for it.
//!
//! This is its own domain because it spans the CLI entry point, the session layer, the UI, and the
//! platform layer, and it outlives the process that starts it. The application operations that
//! drive it live in `ops::relaunch`.

pub mod candidate;
pub mod document;
pub mod plan;
