# Mount a macOS DMG that contains a license agreement.
#
# hdiutil cancels the attach when stdin is not a terminal. The agreement stays
# in the installer; this only answers it for the release check.

import os
import select
import sys
import time


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: attach-macos-dmg.py MOUNT_POINT DMG", file=sys.stderr)
        return 2
    mount, dmg = sys.argv[1], sys.argv[2]
    if not hasattr(os, "forkpty"):
        print("A pseudo-terminal is required to accept the DMG license.", file=sys.stderr)
        return 2
    pid, fd = os.forkpty()
    if pid == 0:
        try:
            os.execvp(
                "hdiutil",
                ["hdiutil", "attach", "-readonly", "-nobrowse", "-mountpoint", mount, dmg],
            )
        finally:
            os._exit(127)

    tail = b""
    sent = False
    deadline = time.monotonic() + 90
    prompts = (b"Agree?", b"[Y/n]", b"[y/N]", b"Y/N?")
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            os.kill(pid, 15)
            print("Timed out while accepting the macOS installer license.", file=sys.stderr)
            break
        ready, _, _ = select.select([fd], [], [], min(remaining, 20))
        if not ready:
            if not sent:
                os.write(fd, b"Y\n")
                sent = True
            continue
        try:
            data = os.read(fd, 4096)
        except OSError:
            break
        if not data:
            break
        sys.stdout.buffer.write(data)
        sys.stdout.buffer.flush()
        tail = (tail + data)[-240:]
        if not sent and any(prompt in tail for prompt in prompts):
            os.write(fd, b"Y\n")
            sent = True
    _, status = os.waitpid(pid, 0)
    return os.waitstatus_to_exitcode(status)


if __name__ == "__main__":
    sys.exit(main())
