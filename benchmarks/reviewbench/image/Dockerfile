# Reviewer image for the ReviewBench agent contract: one container per pull
# request runs the Zeroshot review graph and writes $RB_OUT.
# Build from benchmarks/reviewbench: docker build -f image/Dockerfile -t rb-reviewer:dev .
FROM ghcr.io/the-open-engine/zeroshot-target:10.24.1

LABEL org.opencontainers.image.title="Zeroshot Code Review" \
      org.opencontainers.image.description="Multi-harness Zeroshot code review graph" \
      org.opencontainers.image.source="https://github.com/the-open-engine/zeroshot" \
      org.opencontainers.image.licenses="MIT"

# Harness configuration for every agent: no hosted web search, connectors,
# plugins, browser or computer use. The repository and diff are all an agent reads.
COPY image/home/ /opt/rb/home/
COPY image/entrypoint.py /opt/rb/entrypoint.py
COPY config/ /opt/rb/config/
RUN chmod -R a+rX /opt/rb && chmod 0755 /opt/rb/entrypoint.py

# Runs as root, like ReviewBench's example reviewer, so the mounts are writable whoever owns them.
# Claude Code accepts its permission bypass as root only inside a declared sandbox: the container.
ENV IS_SANDBOX=1
WORKDIR /work
ENTRYPOINT ["python3", "/opt/rb/entrypoint.py"]
