# Mount a macOS DMG that contains a license agreement.
#
# hdiutil cancels the attach when stdin is not a terminal. The agreement stays
# in the installer; this only answers it for the release check. PAGER=cat skips
# the interactive license pager, and every write is non-blocking so a full
# terminal buffer cannot stall the timeout.

import errno
import fcntl
import os
import select
import signal
import struct
import sys

import termios
import time

sys.dont_write_bytecode = True


def configure_terminal(fd: int) -> None:
    flags = fcntl.fcntl(fd, fcntl.F_GETFL)
    fcntl.fcntl(fd, fcntl.F_SETFL, flags | os.O_NONBLOCK)
    try:
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 48, 120, 0, 0))
    except OSError:
        pass


def stop_process(pid: int) -> None:
    for sig in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(pid, sig)
        except ProcessLookupError:
            return
        except OSError:
            try:
                os.kill(pid, sig)
            except ProcessLookupError:
                return
        if sig == signal.SIGTERM:
            time.sleep(0.2)


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
        os.environ["PAGER"] = "cat"
        os.environ["TERM"] = "dumb"
        os.environ["LC_ALL"] = "C"
        try:
            os.execvp(
                "hdiutil",
                ["hdiutil", "attach", "-readonly", "-nobrowse", "-mountpoint", mount, dmg],
            )
        finally:
            os._exit(127)

    configure_terminal(fd)
    deadline = time.monotonic() + 75
    finished = False
    timed_out = False
    try:
        while not finished:
            if time.monotonic() >= deadline:
                timed_out = True
                print(
                    "Timed out while accepting the macOS installer license.",
                    file=sys.stderr,
                )
                break
            try:
                os.write(fd, b"y\n")
            except OSError as error:
                if error.errno not in (errno.EAGAIN, errno.EWOULDBLOCK, errno.EIO):
                    raise
                if error.errno == errno.EIO:
                    break
            remaining = max(0.0, deadline - time.monotonic())
            ready, _, _ = select.select([fd], [], [], min(1.0, remaining))
            if not ready:
                continue
            while True:
                try:
                    data = os.read(fd, 4096)
                except BlockingIOError:
                    break
                except OSError as error:
                    if error.errno == errno.EIO:
                        finished = True
                        break
                    raise
                if not data:
                    finished = True
                    break
                sys.stdout.buffer.write(data)
                sys.stdout.buffer.flush()
    finally:
        stop_process(pid)
        try:
            os.close(fd)
        except OSError:
            pass

    _, status = os.waitpid(pid, 0)
    if timed_out:
        return 1
    return os.waitstatus_to_exitcode(status)


if __name__ == "__main__":
    sys.exit(main())
