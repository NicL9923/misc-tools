#!/usr/bin/env python3
"""Capture one visible app window on an isolated X11 display: NAME OUTPUT.png."""
import os
import subprocess
import sys

if len(sys.argv) != 3 or not os.environ.get('DISPLAY'):
    raise SystemExit('Usage: DISPLAY=:93 capture-x11.py WINDOW_NAME OUTPUT.png')
windows = subprocess.check_output(['xdotool', 'search', '--onlyvisible', '--name', sys.argv[1]], text=True).splitlines()
if len(windows) != 1:
    raise SystemExit(f'Expected one matching window, found {len(windows)}')
geometry = dict(line.split('=', 1) for line in subprocess.check_output(['xdotool', 'getwindowgeometry', '--shell', windows[0]], text=True).splitlines())
subprocess.run(['ffmpeg', '-loglevel', 'error', '-f', 'x11grab', '-video_size', f'{geometry["WIDTH"]}x{geometry["HEIGHT"]}', '-grab_x', geometry['X'], '-grab_y', geometry['Y'], '-i', os.environ['DISPLAY'], '-frames:v', '1', '-update', '1', '-y', sys.argv[2]], check=True)
