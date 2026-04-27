#!/bin/bash -e
# pi-gen calls prerun.sh at the start of each stage. Standard
# pi-gen pattern: copy the previous stage's chroot to this stage's
# work directory so we layer on top.
if [ ! -d "${ROOTFS_DIR}" ]; then
    copy_previous
fi

# Stage the kino .deb into the chroot's /tmp/ so 00-run-chroot.sh
# can `apt-get install -y /tmp/kino.deb`. channels.yml's pi-image
# job downloads the aarch64 .deb from the GitHub Release before
# invoking pi-gen-action and lands it at the repo root, which
# pi-gen-action mounts at ${BASE_DIR}.
if [ -f "${BASE_DIR}/kino.deb" ]; then
    install -Dm644 "${BASE_DIR}/kino.deb" "${ROOTFS_DIR}/tmp/kino.deb"
else
    echo "prerun.sh: ${BASE_DIR}/kino.deb not found — channels.yml's pi-image job must download it before invoking pi-gen-action" >&2
    exit 1
fi
