fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "grpc")]
    {
        let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR")?);

        // Browser render service (server)
        tonic_build::configure()
            .build_server(true)
            .build_client(false)
            .file_descriptor_set_path(out_dir.join("browser_render_descriptor.bin"))
            .compile_protos(&["proto/browser_render.proto"], &["proto"])?;

        // Logi dtakologs service (client for rust-logi)
        tonic_build::configure()
            .build_server(false)
            .build_client(true)
            .file_descriptor_set_path(out_dir.join("logi_descriptor.bin"))
            .compile_protos(
                &["proto/logi/dtakologs.proto", "proto/logi/common.proto"],
                &["proto/logi"],
            )?;
    }
    Ok(())
}
