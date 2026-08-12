#!/bin/sh
set -eu

if [ ! -s /ssh/authorized_keys ]; then
  echo "authorized_keys fixture is missing or empty" >&2
  exit 1
fi

mkdir -p /home/refine/.ssh /run/sshd
cp /ssh/authorized_keys /home/refine/.ssh/authorized_keys
chown -R refine:refine /home/refine/.ssh
chmod 0700 /home/refine/.ssh
chmod 0600 /home/refine/.ssh/authorized_keys

ssh-keygen -A >/dev/null

exec /usr/sbin/sshd -D -e -f /etc/ssh/sshd_config
