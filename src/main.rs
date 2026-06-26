use atv_iptv_proxy::server;

fn main() {
    let args = match server::parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("atv-iptv-proxy: {err}");
            std::process::exit(2);
        }
    };

    if let Err(err) = server::run(args) {
        eprintln!("atv-iptv-proxy: {err}");
        std::process::exit(1);
    }
}
