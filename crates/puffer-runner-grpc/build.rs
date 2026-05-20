fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto = "proto/tool_runner.proto";
    println!("cargo:rerun-if-changed={proto}");
    tonic_build::configure()
        .build_client(true)
        .build_server(true)
        .protoc_arg("--experimental_allow_proto3_optional")
        .compile_protos(&[proto], &["proto"])?;
    Ok(())
}
