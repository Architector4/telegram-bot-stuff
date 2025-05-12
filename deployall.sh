#!/bin/sh

for i in */; do
	(
		cd "$i" || return
		./deploy.sh
	)
done
