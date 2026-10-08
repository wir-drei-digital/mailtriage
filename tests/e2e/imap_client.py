#!/usr/bin/env python3
"""IMAP helper for the Dovecot end-to-end test (standard library only).

It plays the mail client and the test's eyes: it seeds mail, finds where a
message is, and moves or deletes mail the way a user would. Every subcommand
prints JSON on stdout; errors go to stderr with exit status 1.

    imap_client.py --port PORT ping
    imap_client.py --port PORT append FOLDER FILE
    imap_client.py --port PORT locate MESSAGE_ID
    imap_client.py --port PORT move FOLDER MESSAGE_ID TARGET
    imap_client.py --port PORT delete FOLDER MESSAGE_ID
    imap_client.py --port PORT reset-epoch FOLDER CONTAINER
"""
import argparse
import imaplib
import json
import re
import subprocess
import sys
import time

USER = "e2e"
PASSWORD = "e2e-pass"

LIST_LINE = re.compile(rb'^\((?P<attrs>[^)]*)\) (?P<delim>NIL|"(?:[^"\\]|\\.)*") ?(?P<name>.*)$')
UIDVALIDITY = re.compile(rb"UIDVALIDITY (\d+)")


def quote(text):
    """An IMAP quoted string; Python 3's imaplib sends arguments verbatim."""
    return '"' + text.replace("\\", "\\\\").replace('"', '\\"') + '"'


def unquote(raw):
    raw = raw.strip()
    if raw.startswith(b'"') and raw.endswith(b'"') and len(raw) >= 2:
        return re.sub(rb"\\(.)", rb"\1", raw[1:-1])
    return raw


def check(result, what):
    typ, data = result
    if typ != "OK":
        raise RuntimeError(f"{what} failed: {typ} {data!r}")
    return data


def connect(args):
    conn = imaplib.IMAP4(args.host, args.port, timeout=30)
    check(conn.login(USER, PASSWORD), "LOGIN")
    return conn


def mailboxes(conn):
    """Every selectable mailbox, in LIST order."""
    names = []
    for item in check(conn.list('""', '"*"'), "LIST"):
        if not item:
            continue
        if isinstance(item, tuple):
            line, name = item[0], item[1]
        else:
            line, name = item, None
        match = LIST_LINE.match(line)
        if not match:
            raise RuntimeError(f"unexpected LIST line {line!r}")
        attrs = match.group("attrs").lower().split()
        if b"\\noselect" in attrs or b"\\nonexistent" in attrs:
            continue
        if name is None:
            name = unquote(match.group("name"))
        names.append(name.decode("ascii"))
    return names


def find_uids(conn, message_id):
    data = check(conn.uid("SEARCH", "HEADER", "Message-ID", quote(message_id)), "UID SEARCH")
    return [int(uid) for uid in b" ".join(d for d in data if d).split()]


def one_uid(conn, folder, message_id):
    uids = find_uids(conn, message_id)
    if len(uids) != 1:
        raise RuntimeError(f"expected one {message_id} in {folder}, found UIDs {uids}")
    return uids[0]


def select(conn, folder, readonly=False):
    check(conn.select(quote(folder), readonly=readonly), "EXAMINE" if readonly else "SELECT")


def flags_of(conn, uid):
    data = check(conn.uid("FETCH", str(uid), "(FLAGS)"), "UID FETCH")
    for item in data:
        line = item[0] if isinstance(item, tuple) else item
        if line and b"FLAGS" in line:
            return sorted(f.decode("ascii") for f in imaplib.ParseFlags(line))
    raise RuntimeError(f"no FLAGS for UID {uid}")


def uid_validity(conn, folder):
    data = check(conn.status(quote(folder), "(UIDVALIDITY)"), "STATUS")
    match = UIDVALIDITY.search(b" ".join(d for d in data if isinstance(d, bytes)))
    if not match:
        raise RuntimeError(f"STATUS {folder} lacks UIDVALIDITY: {data!r}")
    return int(match.group(1))


def cmd_ping(conn, args):
    check(conn.noop(), "NOOP")
    return {"ok": True}


def cmd_append(conn, args):
    with open(args.file, "rb") as f:
        message = f.read()
    # No flags (so not \Seen) and the server's own INTERNALDATE.
    data = check(conn.append(quote(args.folder), None, None, message), "APPEND")
    return {"folder": args.folder, "response": [d.decode("ascii", "replace") for d in data if d]}


def cmd_locate(conn, args):
    found = []
    for folder in mailboxes(conn):
        select(conn, folder, readonly=True)  # EXAMINE: never changes flags
        for uid in find_uids(conn, args.message_id):
            found.append([folder, uid, flags_of(conn, uid)])
        conn.close()
    return found


def cmd_move(conn, args):
    select(conn, args.folder)
    uid = one_uid(conn, args.folder, args.message_id)
    data = check(conn.uid("MOVE", str(uid), quote(args.target)), "UID MOVE")
    return {"folder": args.folder, "uid": uid, "target": args.target,
            "response": [d.decode("ascii", "replace") for d in data if isinstance(d, bytes)]}


def cmd_delete(conn, args):
    # The simulated user deleting mail; mailtriage itself never does this.
    select(conn, args.folder)
    uid = one_uid(conn, args.folder, args.message_id)
    check(conn.uid("STORE", str(uid), "+FLAGS.SILENT", "(\\Deleted)"), "UID STORE")
    check(conn.expunge(), "EXPUNGE")
    return {"folder": args.folder, "uid": uid}


def cmd_answer(conn, args):
    # The simulated user replying: mail clients set \Answered on the original.
    select(conn, args.folder)
    uid = one_uid(conn, args.folder, args.message_id)
    check(conn.uid("STORE", str(uid), "+FLAGS.SILENT", "(\\Answered)"), "UID STORE")
    return {"folder": args.folder, "uid": uid}


def cmd_reset_epoch(conn, args):
    old = uid_validity(conn, args.folder)
    # Dovecot hands out UIDVALIDITY values from a counter that can run ahead
    # of the clock, so "now" alone might not change it.
    new = max(int(time.time()), old + 1)
    subprocess.run(
        ["docker", "exec", args.container, "doveadm", "mailbox", "update",
         "-u", USER, "--uid-validity", str(new), args.folder],
        check=True, stdin=subprocess.DEVNULL, stdout=sys.stderr,
    )
    current = uid_validity(conn, args.folder)
    if current != new:
        raise RuntimeError(f"UIDVALIDITY of {args.folder} is {current} after setting {new}")
    return {"folder": args.folder, "old": old, "new": new}


def parse_args(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, required=True)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("ping").set_defaults(run=cmd_ping)
    p = sub.add_parser("append")
    p.add_argument("folder")
    p.add_argument("file")
    p.set_defaults(run=cmd_append)
    p = sub.add_parser("locate")
    p.add_argument("message_id")
    p.set_defaults(run=cmd_locate)
    p = sub.add_parser("move")
    p.add_argument("folder")
    p.add_argument("message_id")
    p.add_argument("target")
    p.set_defaults(run=cmd_move)
    p = sub.add_parser("delete")
    p.add_argument("folder")
    p.add_argument("message_id")
    p.set_defaults(run=cmd_delete)
    p = sub.add_parser("answer")
    p.add_argument("folder")
    p.add_argument("message_id")
    p.set_defaults(run=cmd_answer)
    p = sub.add_parser("reset-epoch")
    p.add_argument("folder")
    p.add_argument("container")
    p.set_defaults(run=cmd_reset_epoch)
    return parser.parse_args(argv)


def main(argv):
    args = parse_args(argv)
    try:
        conn = connect(args)
        try:
            result = args.run(conn, args)
        finally:
            try:
                conn.logout()
            except (imaplib.IMAP4.error, OSError):
                pass
    except (imaplib.IMAP4.error, OSError, RuntimeError, subprocess.CalledProcessError) as e:
        print(f"imap_client {args.command}: {e}", file=sys.stderr)
        return 1
    json.dump(result, sys.stdout)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
