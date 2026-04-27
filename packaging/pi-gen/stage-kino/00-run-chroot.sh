#!/bin/bash -e
# Runs inside the Pi OS chroot — installs the kino .deb that the
# release pipeline produced for aarch64, enables the systemd unit
# (without starting it; the user does that on first boot), and
# sets the hostname to kino so mDNS publishes kino.local.
#
# The .deb is staged into the chroot's /tmp by the release workflow
# before pi-gen-action runs (see the pi-image job in
# .github/workflows/channels.yml).

if [ ! -f /tmp/kino.deb ]; then
    echo "expected /tmp/kino.deb to be staged before pi-gen runs" >&2
    exit 1
fi

apt-get install -y /tmp/kino.deb
rm /tmp/kino.deb

systemctl enable kino.service

# Default hostname — discoverable as kino.local via Avahi. Users
# can override in Pi Imager's cloud-init step before flashing.
echo kino > /etc/hostname

# Unattended-upgrades for between-release CVE patching. Debian
# security team handles upstream; this picks them up automatically.
cat > /etc/apt/apt.conf.d/51unattended-upgrades-kino <<'EOF'
APT::Periodic::Update-Package-Lists "1";
APT::Periodic::Unattended-Upgrade "1";
EOF
