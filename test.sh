#!/bin/sh
for i in $(seq 100000)
do
	echo "loop"
	cargo test -- --nocapture --exact
done
