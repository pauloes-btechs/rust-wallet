//! Generates gRPC clients from the vendored Lightning Labs protos in
//! `/proto` (pinned to litd v0.17.8; see proto/upstream.json). protoc comes
//! from `protoc-bin-vendored`, so no system install is needed.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // SAFETY-free in edition 2021: single-threaded build script.
    std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);
    let dir = "../../proto";
    let protos = [
        "lightning.proto",
        "router.proto",
        "walletunlocker.proto",
        "lit-status.proto",
        "taprootassets.proto",
        "tapcommon.proto",
        "loop.proto",
        "common.proto",
        "faraday.proto",
    ]
    .map(|p| format!("{dir}/{p}"));
    for p in &protos {
        println!("cargo:rerun-if-changed={p}");
    }
    tonic_prost_build::configure().build_server(false).compile_protos(&protos, &[dir.to_string()])?;
    Ok(())
}
