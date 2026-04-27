#!/bin/sh
# RPM post-install — mirrors the .deb postinst. Creates the kino
# system user + data dirs, enables the unit but doesn't start it (the
# operator decides), prints a "where to go next" hint on first
# install only ($1 == 1).

set -e

if ! id kino >/dev/null 2>&1; then
    useradd --system --user-group --home-dir /var/lib/kino \
        --no-create-home --shell /sbin/nologin kino || true
fi

mkdir -p /var/lib/kino /etc/kino
chown -R kino:kino /var/lib/kino /etc/kino
chmod 750 /var/lib/kino /etc/kino

systemctl daemon-reload || true
systemctl enable kino.service || true

if [ "$1" = "1" ]; then
    cat <<'EOF'

  Kino installed.

  Start the service:    sudo systemctl start kino
  Open the web UI:      http://localhost:8080
  Logs:                 sudo journalctl -u kino -f

  See https://kinostack.app for setup help.

EOF
fi

exit 0
