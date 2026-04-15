#!/usr/bin/env bash

# Personal hacky script to get it all done and notify me about it lol

for i in . */; do
	(
		cd "$i" || return
		chrt -i 0 ionice -c 3 cargo check-all-features -- --keep-going&
		chrt -i 0 ionice -c 3 cargo clippy --all-targets --keep-going&
		chrt -i 0 ionice -c 3 cargo build --keep-going&
		chrt -i 0 ionice -c 3 cargo build --release --keep-going&
		chrt -i 0 ionice -c 3 cargo test&
		wait
		)&
done
wait
notify-send compiled
