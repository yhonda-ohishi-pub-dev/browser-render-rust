fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "grpc")]
    {
        let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
        tonic_build::configure()
            .build_server(true)
            .build_client(false)
            .file_descriptor_set_path(out_dir.join("browser_render_descriptor.bin"))
            .compile_protos(&["proto/browser_render.proto"], &["proto"])?;
    }
    Ok(())
}
