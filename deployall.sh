#!/bin/sh

for i in */deploy.sh; do
	"$i"
done
