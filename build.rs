fn main() {
    println!("cargo:rerun-if-changed=assets/brand/StarSyncMinerScan.ico");
    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/brand/StarSyncMinerScan.ico");
        res.compile()
            .expect("failed to embed MinerScan application icon");
    }
}
