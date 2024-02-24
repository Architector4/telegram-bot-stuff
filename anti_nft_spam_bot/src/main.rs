use arch_bot_commons::*;

fn main() {
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "WARNING,anti_nft_spam_bot=debug");
    }
    start_everything(anti_nft_spam_bot::entry());
}
