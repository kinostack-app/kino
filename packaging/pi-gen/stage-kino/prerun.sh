#!/bin/bash -e
# pi-gen calls prerun.sh at the start of each stage. Standard
# pi-gen pattern: copy the previous stage's chroot to this stage's
# work directory so we layer on top.
if [ ! -d "${ROOTFS_DIR}" ]; then
    copy_previous
fi
