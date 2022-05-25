#! /usr/bin/env python3

import os
import pwd
import struct
import sys
import subprocess
import socket
import logging
import time
from argparse import ArgumentParser
from pwd import getpwnam
import grp

log = logging.getLogger("video-server")
console_handler = logging.StreamHandler()
console_handler.setFormatter(logging.Formatter("%(levelname)-5s: %(message)s"))
log.addHandler(console_handler)
log.setLevel(logging.INFO)

PATH_XINIT = "/usr/bin/xinit"
PATH_XVFB = "/usr/bin/Xvfb"
PATH_RUN_X_ENV = "/usr/local/bin/run_x_env.sh"
PATH_SANZU_SERVER = "/usr/bin/sanzu_server"
PATH_SANZU_CONFIG = "/etc/sanzu.toml"

def run_daemon(args, env=None):
    # Quick and diry run & detach process
    # May be replaced by systemd lingers
    pid1 = os.fork()
    if pid1 == 0:
        pid2 = os.fork()
        if pid2 == 0:
            process = subprocess.Popen(args, env=env)
            log.info(process)

            process.wait()
            log.info("son terminated")
        else:
            os._exit(0)
    else:
        os.wait()


def check_xinit(uid):
    # Quick & dirty: Look for runinng xinit for a given user
    process = subprocess.Popen(["pgrep", "-u", "%d" % uid, "xinit"], stdout=subprocess.PIPE)
    ret = process.wait()
    if ret != 0:
        return False
    stdout = process.stdout.readlines()
    # Test output
    if len(stdout) == 0:
        # Should not happen
        return False
    return True


if __name__ == '__main__':

    parser = ArgumentParser("run_video_server.py")
    parser.add_argument('username', help="Username")
    parser.add_argument('unixsocket', help="unixsocket path")
    parser.add_argument('-v', "--verbose", action="count", help="Verbose mode",
                        default=0)
    args = parser.parse_args()

    if args.verbose:
        log.setLevel(logging.DEBUG)

    uid = getpwnam(args.username).pw_uid
    gid = getpwnam(args.username).pw_gid

    log.info("uid: %d gid: %d" % (uid, gid))

    log.debug('Change socket owner')
    os.chown(args.unixsocket, uid, gid)

    log.debug('Set as user')

    # set groups
    groups = [g.gr_name for g in grp.getgrall() if args.username in g.gr_mem]
    groups.append(grp.getgrgid(gid).gr_name)

    gids = []
    for group in groups:
        gid = grp.getgrnam(group).gr_gid
        gids.append(gid)

    # Will raise error on failure
    os.setgroups(gids)
    os.setgid(gid)
    os.setuid(uid)


    # Set env
    home = pwd.getpwuid(uid).pw_dir
    os.chdir(home)

    display_name = ":%d" % uid
    env={
        "RUST_LOG": "info",
        "DISPLAY": display_name,
        "USERNAME": args.username,
        "USER": args.username,
        "HOME": home,
    }

    # Prepare filename if sanzu video server uses memory mapped file as graphic input
    screen_dir = "/var/tmp/%d/" % uid
    screen_path = os.path.join(screen_dir, "Xvfb_screen0")

    if not check_xinit(uid):
        log.info('Start xinit')
        try:
            os.mkdir(screen_dir)
        except:
            pass
        xinit_args = [
            PATH_XINIT,
            PATH_RUN_X_ENV,
            "--",
            PATH_XVFB, display_name, "-screen", "0", "4096x4096x24", "-fbdir", screen_dir,
            "-nocursor",
        ]
        run_daemon(
            xinit_args,
            env=env
        )
        time.sleep(0.5)

    log.info('Start video server')



    server_args = [
        PATH_SANZU_SERVER,
        "-u", "-c", "-l", args.unixsocket,
        "-f", PATH_SANZU_CONFIG,
        '-a',
        '-s',
        "-e", "libx264",
        "-k", screen_path,
    ]

    run_daemon(
        server_args,
        env=env
    )
