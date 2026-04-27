#!/bin/sh
# RPM post-uninstall — runs after the .rpm is removed.
#
# $1 carries the count of remaining installations after this op:
#   - $1 == 0 : final uninstall (last copy gone)
#   - $1 == 1 : upgrade in progress (new pkg already installed,
#               this is the old one being cleaned up)
#
# We stop + disable the service on final uninstall and reload
# systemd so the orphaned unit reference goes away. We do NOT
# remove /var/lib/kino, /etc/kino, or the kino user — RPM has
# no equivalent of `apt purge`, and the user expects that
# `dnf reinstall kino` later will pick up where they left off.
# Manual cleanup if the user really wants a fresh slate:
#
#   sudo rm -rf /var/lib/kino /etc/kino
#   sudo userdel kino  (group goes with it)

set -e

if [ "$1" = "0" ]; then
    # Stop the service if it's running. `--quiet` so a non-running
    # service doesn't spam stderr; `|| true` so a failure here
    # doesn't abort the rest of the uninstall.
    systemctl stop kino.service --quiet || true
    systemctl disable kino.service --quiet || true
    systemctl daemon-reload || true
fi

exit 0
