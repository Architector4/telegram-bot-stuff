use arch_bot_commons::*;

fn main() {
    if std::env::var_os("RUST_LOG").is_none() {
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("RUST_LOG", "WARN,teco_tools_bot=debug") };
    }
    start_everything(teco_tools_bot::entry());
}
