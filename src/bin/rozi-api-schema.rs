//! Regenerate the checked-in control API JSON Schema.
//!
//! ```text
//! cargo run --features schema-gen --bin rozi-api-schema
//! ```
//!
//! Writes [`rozi::api_schema::SCHEMA_PATH`] relative to the current directory, which is the
//! repository root when run through Cargo. CI runs this and fails on a diff, so the published
//! schema cannot drift from the types that produce the JSON.

fn main() -> std::io::Result<()> {
    let path = std::path::Path::new(rozi::api_schema::SCHEMA_PATH);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, rozi::api_schema::document())?;
    println!("wrote {}", path.display());
    Ok(())
}
