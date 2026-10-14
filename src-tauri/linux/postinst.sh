#!/bin/sh
# Probes other than ICMP (ping socket) go out through raw sockets. This capability lets
# every user open them without running TracePlotter as root, as trippy's packages do.
setcap cap_net_raw+ep /usr/bin/traceplotter || true
