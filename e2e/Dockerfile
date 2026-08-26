# systemd-in-docker template for pandemic E2E testing.
#
# Gives you a real init system (PID 1 = /sbin/init) inside a container:
# real systemctl, real units, real journald, real cgroup management —
# without touching the host (no sudo, no KVM). See README.md.
#
# Base is ubuntu:24.04: multi-arch (aarch64 + amd64), glibc 2.39, and the
# systemd package is complete. Requires a cgroup v2 host (default on modern
# distros).
FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive

# systemd-sysv provides /sbin/init; procps/less/curl for debugging.
RUN apt-get update && apt-get install -y --no-install-recommends \
      systemd systemd-sysv dbus procps less curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Mask units that cannot or should not run inside a container.
# /dev/null symlinks are the standard mask mechanism.
RUN mkdir -p /etc/systemd/system && for u in \
      systemd-udevd.service \
      systemd-networkd.service \
      systemd-resolved.service \
      systemd-timesyncd.service \
      systemd-logind.service \
      systemd-oomd.service \
      systemd-firstboot.service \
      setvtr.sh.service \
      systemd-journal-flush.service \
      getty.target \
      apt-daily.service \
      apt-daily.timer \
      apt-daily-upgrade.service \
      apt-daily-upgrade.timer \
      e2fsck@.service \
      e2fsck.timer \
      fstrim.timer \
      sys-kernel-debug.mount ; do \
      ln -sf /dev/null /etc/systemd/system/$u; done

# systemd state (e.g. volatile journal) — optional anonymous volume.
VOLUME /var/lib/systemd

# SIGRTMIN+3 = systemd "flush journal and power off", so `docker stop`
# shuts the init system down gracefully instead of SIGKILLing it.
STOPSIGNAL SIGRTMIN+3

CMD ["/sbin/init"]
