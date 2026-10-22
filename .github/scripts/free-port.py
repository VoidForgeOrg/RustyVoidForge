#!/usr/bin/env python3
"""Prints a TCP port for a host-network container's listener (P5.2, D-065): free on this host, and
from 20000-29999, outside the ephemeral ranges of Linux (32768-60999) and macOS (49152-65535). On
macOS the container runs in colima's VM, whose own outgoing connections to 127.0.0.1 (to Postgres,
say) take ephemeral local ports there: a port the kernel picks free on the Mac can be taken in the
VM (P5.2's table B: "bind 127.0.0.1:54192: Address already in use").
"""
import random
import socket

for _ in range(200):
    port = random.randint(20000, 29999)
    with socket.socket() as s:
        try:
            s.bind(("127.0.0.1", port))
        except OSError:
            continue
    print(port)
    break
else:
    raise SystemExit("no free port in 20000-29999")
