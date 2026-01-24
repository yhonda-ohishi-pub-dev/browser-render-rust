pub mod browser;
pub mod config;
pub mod jobs;
pub mod server;
pub mod storage;

// rust-logi gRPC client (generated from proto)
#[cfg(feature = "grpc")]
pub mod logi {
    pub mod common {
        tonic::include_proto!("logi.common");
    }
    pub mod dtakologs {
        tonic::include_proto!("logi.dtakologs");
    }
}
