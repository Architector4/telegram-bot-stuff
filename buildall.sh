#!/usr/bin/env bash

# Personal hacky script to get it all done and notify me about it lol

for i in */; do
	(
		cd "$i" || return
		cargo clippy&
		cargo build&
		cargo build --release&
		cargo test&
		wait
		)&
done
wait
notify-send compiled
