#!/usr/bin/env bash
exec ./udpprox.exe -b 0.0.0.0 -r $2 -l $2 --host $1 & echo "pid = $!"
