#!/usr/bin/env python3
"""Native-protocol fixture with a child that escapes process-group containment."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys


def emit(value):
    print(json.dumps(value), flush=True)


emit({"type": "ready", "protocol": 1})
init = json.loads(sys.stdin.readline())
emit({"type": "initialized", "sessionId": init["sessionId"]})
child = None
try:
    for line in sys.stdin:
        message = json.loads(line)
        if message["type"] == "stop":
            break
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"],
                                 start_new_session=True)
        Path("owned-child.pid").write_text(str(child.pid))
finally:
    if child is not None:
        os.killpg(child.pid, signal.SIGKILL)
        child.wait(timeout=2)
    emit({"type": "usage", "usage": None})
    emit({"type": "closed", "clean": True})
