FROM alpine:3.20

RUN apk add --no-cache git openssh-server \
    && adduser -D -s /bin/sh refine \
    && echo 'refine:refine' | chpasswd \
    && mkdir -p /run/sshd /ssh /home/refine/.ssh

COPY entrypoint.sh /usr/local/bin/refine-cluster-ssh-entrypoint
COPY sshd_config /etc/ssh/sshd_config

RUN chmod 0755 /usr/local/bin/refine-cluster-ssh-entrypoint

EXPOSE 22

ENTRYPOINT ["/usr/local/bin/refine-cluster-ssh-entrypoint"]
