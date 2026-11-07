//! Regenerate the checked-in control API and session wire JSON Schemas.
//!
//! ```text
//! cargo run --features schema-gen --bin rozi-api-schema
//! ```
//!
//! Writes [`rozi::api_schema::SCHEMA_PATH`] and
//! [`rozi::session::protocol::wire_schema::SCHEMA_PATH`] relative to the current directory, which
//! is the repository root when run through Cargo. CI runs this and fails on a diff, so neither
//! schema can drift from the types that produce the JSON.

fn main() -> std::io::Result<()> {
    write(rozi::api_schema::SCHEMA_PATH, rozi::api_schema::document())?;
    write(
        rozi::session::protocol::wire_schema::SCHEMA_PATH,
        rozi::session::protocol::wire_schema::document(),
    )
}

fn write(path: &str, document: String) -> std::io::Result<()> {
    let path = std::path::Path::new(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, document)?;
    println!("wrote {}", path.display());
    Ok(())
}
