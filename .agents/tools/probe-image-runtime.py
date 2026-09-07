#!/usr/bin/env python3
"""Generate a real 1024px image with each installed model and report elapsed time."""
import argparse
import json
from pathlib import Path
import time
import urllib.request
import uuid

REPO = Path(__file__).resolve().parents[2]
BASE = 'http://127.0.0.1:8190'


def api(path, data=None):
    request = urllib.request.Request(BASE + path, data=None if data is None else json.dumps(data).encode(), headers={'Content-Type': 'application/json'})
    with urllib.request.urlopen(request, timeout=30) as response:
        body = response.read()
        return json.loads(body) if body else None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('models', nargs='*', choices=['klein', 'z-image'], default=['klein', 'z-image'])
    args = parser.parse_args()
    queue = api('/queue')
    if queue['queue_running'] or queue['queue_pending']:
        raise SystemExit('Backend has queued work; finish that before probing.')
    for model in args.models:
        api('/free', {'unload_models': True, 'free_memory': True})
        graph = json.loads((REPO / f'apps/image-studio/workflows/{model}.json').read_text())
        prompt_id = str(uuid.uuid4())
        started = time.monotonic()
        response = api('/prompt', {'prompt': graph, 'prompt_id': prompt_id, 'client_id': 'image-studio-runtime-probe'})
        assert response['prompt_id'] == prompt_id, response
        print(f'Started {model}: {prompt_id}', flush=True)
        deadline = started + 1800
        while time.monotonic() < deadline:
            history = api('/history/' + prompt_id)
            if prompt_id in history:
                result = history[prompt_id]
                print(json.dumps({'model': model, 'elapsed_seconds': round(time.monotonic() - started, 2), 'status': result['status'], 'outputs': result['outputs']}, indent=2), flush=True)
                if result['status']['status_str'] != 'success':
                    raise SystemExit(1)
                break
            time.sleep(1)
        else:
            raise SystemExit(f'Timed out waiting for {prompt_id}; check the backend before retrying.')
    api('/free', {'unload_models': True, 'free_memory': True})


if __name__ == '__main__':
    main()
