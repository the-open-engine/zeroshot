# An agent image whose `codex` command runs Codex under codex-reaper.c (experiments that set
# resources.reap_orphans). Built from the experiment's agent image with the task image's own C
# compiler; the harness files are otherwise unchanged.
ARG AGENT_IMAGE
FROM ${AGENT_IMAGE}
USER root
COPY codex-reaper.c /tmp/codex-reaper.c
RUN cc -O2 -Wall -Wextra -Werror -o /tmp/codex-reaper /tmp/codex-reaper.c \
    && rm /usr/local/bin/codex /tmp/codex-reaper.c \
    && install -o root -g root -m 0711 /tmp/codex-reaper /usr/local/bin/codex \
    && rm /tmp/codex-reaper \
    && codex --version
WORKDIR /workspace
CMD ["sleep", "infinity"]
