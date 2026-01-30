#!/usr/bin/env bash

# Personal hacky script to get it all done and notify me about it lol

for i in . */; do
	(
		cd "$i" || return
		chrt -i 0 ionice -c 3 cargo clippy&
		chrt -i 0 ionice -c 3 cargo clippy --all-targets&
		chrt -i 0 ionice -c 3 cargo build&
		chrt -i 0 ionice -c 3 cargo build --release&
		chrt -i 0 ionice -c 3 cargo test&
		wait
		)&
done
wait
notify-send compiled
