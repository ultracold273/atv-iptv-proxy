use atv_iptv_proxy::server;

fn main() {
    if let Err(err) = server::run_from_env() {
        eprintln!("atv-iptv-proxy: {err}");
        std::process::exit(1);
    }
}
