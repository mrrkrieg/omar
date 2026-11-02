"""Record real OMAR CLI results and render a captioned, condensed GIF replay."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import subprocess
import tempfile
import textwrap
import time

from PIL import Image, ImageDraw, ImageFont

parser = argparse.ArgumentParser()
parser.add_argument('--binary', required=True)
parser.add_argument('--output', required=True)
parser.add_argument('--commit', required=True)
args = parser.parse_args()
out = Path(args.output)
out.mkdir(parents=True, exist_ok=True)
binary = Path(args.binary).resolve()
trace = {'commit': args.commit, 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
         'presentation': 'Real CLI output; JSON condensed; waits shortened. No Mission Control UI.',
         'setup': [], 'scenes': []}

def short_record(value):
    return [f'scope: {value["scope"]["kind"]}  |  status: {value["value"]["status"]}  |  revision: {value["value"]["revision"]}',
            json.dumps({'text': value['value']['text']}, ensure_ascii=False)]

with tempfile.TemporaryDirectory(prefix='omar-memory-video-') as folder:
    env = {k: v for k, v in os.environ.items() if not k.startswith('OMAR_') and k not in ('TMUX', 'TMUX_PANE')}
    env.update(OMAR_HOME=str(Path(folder) / 'state'), PATH=str(binary.parent) + os.pathsep + env['PATH'], TERM='xterm-256color')

    def invoke(command, expected=0, projection=None, public=True):
        start = time.monotonic()
        result = subprocess.run(['/bin/bash', '--noprofile', '--norc', '-c', command], cwd=folder,
                                env=env, text=True, capture_output=True, timeout=90)
        assert result.returncode == expected, (command, result.stdout, result.stderr)
        data = json.loads(result.stdout) if result.returncode == 0 else None
        entry = {'command': command, 'exit_code': result.returncode, 'elapsed_seconds': round(time.monotonic()-start, 3)}
        if public:
            entry.update(stdout=result.stdout, stderr=result.stderr)
        entry['lines'] = projection(data) if projection else result.stderr.strip().splitlines()
        return entry, data

    try:
        _, started = invoke('omar up -s memory-demo --no-ea --json', public=False)
        trace['setup'].append({'command': 'omar up -s memory-demo --no-ea --json', 'state': started['state']})
        env['OMAR_SESSION_ID'] = 'memory-demo'
        for name in ['Writer', 'Related', 'Other']:
            entry, data = invoke(f'omar ea create --name {name}')
            trace['setup'].append(entry)
        entry, project = invoke('omar --ea Writer memory project-create Research')
        trace['setup'].append(entry)
        env['project_id'] = project['id']
        for ea in ['Writer', 'Related']:
            entry, _ = invoke(f'omar --ea {ea} memory project-link "$project_id"')
            trace['setup'].append(entry)

        save = 'omar --ea Writer memory save --scope project \\\n  --key report-format --kind preference \\\n  --text "Use five sections" --source "User request"'
        saved, data = invoke(save, projection=short_record)
        assert data['value']['status'] == 'confirmed'
        env['memory_id'] = data['id']
        trace['scenes'].append({'title': 'Save a preference once',
            'subtitle': 'A project-scoped record with a source and a revision.',
            'duration': 6, 'events': [saved]})

        related, data = invoke('omar --ea Related memory recall',
            projection=lambda v: [json.dumps([r['value']['text'] for r in v['records']])])
        assert data['records'][0]['value']['text'] == 'Use five sections'
        other, data = invoke('omar --ea Other memory recall', projection=lambda v: [json.dumps(v['records'])])
        assert data['records'] == []
        trace['scenes'].append({'title': 'Share only with linked chats',
            'subtitle': 'Related shares the project. Other does not.',
            'duration': 6, 'events': [related, other]})

        edit = 'omar --ea Related memory edit "$memory_id" \\\n  --revision 1 --text "Use three sections" \\\n  --source "User correction"'
        updated, data = invoke(edit, projection=short_record)
        assert data['value']['revision'] == 2
        stale = 'omar --ea Writer memory edit "$memory_id" \\\n  --revision 1 --text "Stale overwrite" --source "Old edit"'
        rejected, _ = invoke(stale, expected=1)
        assert 'revision conflict' in rejected['stderr']
        trace['scenes'].append({'title': 'Correct it without losing newer work',
            'subtitle': 'Revision 2 wins. An edit based on revision 1 is rejected.',
            'duration': 8, 'events': [updated, rejected]})

        forgotten, data = invoke('omar --ea Related memory forget "$memory_id" --revision 2',
            projection=lambda v: [f'status: {v["value"]["status"]}'])
        empty, data = invoke('omar --ea Related memory recall', projection=lambda v: [json.dumps(v['records'])])
        assert data['records'] == []
        retry, _ = invoke(save, expected=1)
        assert 'forgotten' in retry['stderr']
        trace['scenes'].append({'title': 'Forget it and block accidental return',
            'subtitle': 'Future recall is empty. Retrying the same saved key is blocked.',
            'duration': 9, 'events': [forgotten, empty, retry]})
    finally:
        subprocess.run(['omar', 'rm', '-s', 'memory-demo'], env=env, cwd=folder,
                       capture_output=True, text=True, timeout=90)

(out/'recording.json').write_text(json.dumps(trace, ensure_ascii=False, indent=2)+'\n')

W, H = 1120, 640
BG, PANEL, TEXT, MUTED, ACCENT, GREEN, RED = '#10131b', '#191e29', '#e9edf5', '#9ba8bd', '#b8a4ff', '#8be0b0', '#ffad9f'
sans = '/System/Library/Fonts/Supplemental/Arial.ttf'
bold = '/System/Library/Fonts/Supplemental/Arial Bold.ttf'
mono = '/System/Library/Fonts/Menlo.ttc'
fonts = {k: ImageFont.truetype(p, n) for k, p, n in [('label',bold,17),('title',bold,31),('sub',sans,20),('mono',mono,19),('small',sans,16)]}

def wrapped(line, width=85):
    return textwrap.wrap(line, width=width, subsequent_indent='  ', replace_whitespace=False) or ['']

def frame(scene_index, visible, command_only=False):
    scene = trace['scenes'][scene_index]
    im = Image.new('RGB', (W, H), BG)
    d = ImageDraw.Draw(im)
    d.text((40,26), 'OMAR  /  SCOPED MEMORY', font=fonts['label'], fill=ACCENT)
    d.text((940,26), f'{scene_index+1} / 4', font=fonts['label'], fill=MUTED)
    d.text((40,65), scene['title'], font=fonts['title'], fill=TEXT)
    d.text((40,110), scene['subtitle'], font=fonts['sub'], fill=MUTED)
    d.rounded_rectangle((40,157,W-40,590), radius=15, fill=PANEL)
    d.line((40,204,W-40,204), fill='#30384a', width=1)
    d.text((61,174),'CLI DEMO',font=fonts['label'],fill=TEXT)
    d.text((205,174),'Writer + Related → Research    ·    Other → unlinked',font=fonts['small'],fill=MUTED)
    y=225
    for index, event in enumerate(scene['events'][:visible]):
        for line_index, line in enumerate(event['command'].splitlines()):
            for part in wrapped(('$ ' if line_index == 0 else '  ') + line):
                d.text((62,y),part,font=fonts['mono'],fill=TEXT); y+=25
        if not (command_only and index == visible-1):
            for line in event['lines']:
                for part in wrapped(line):
                    d.text((62,y),part,font=fonts['mono'],fill=RED if event['exit_code'] else GREEN); y+=25
        y += 18
    assert y <= 590, (scene_index, y)
    d.text((40,610),'Real runtime • JSON condensed • waits shortened (sped up)',font=fonts['small'],fill=MUTED)
    d.text((938,610),args.commit[:7],font=fonts['small'],fill=MUTED)
    return im

frames, durations = [], []
for scene_index, scene in enumerate(trace['scenes']):
    per_event = scene['duration']/len(scene['events'])
    for visible in range(1,len(scene['events'])+1):
        frames.append(frame(scene_index,visible,True)); durations.append(500)
        completed=frame(scene_index,visible)
        frames.append(completed); durations.append(round((per_event-.5)*1000))
    completed.save(out/f'scene-{scene_index+1}.png')

# One palette for the whole recording avoids color flicker and keeps it small.
colors = [tuple(bytes.fromhex(c.lstrip('#'))) for c in [BG, PANEL]]
for background in [BG, PANEL]:
    base = tuple(bytes.fromhex(background.lstrip('#')))
    for color in [TEXT, MUTED, ACCENT, GREEN, RED]:
        foreground = tuple(bytes.fromhex(color.lstrip('#')))
        for step in range(24):
            colors.append(tuple(round(base[k] + (foreground[k]-base[k])*step/23) for k in range(3)))
colors += [colors[0]] * (256-len(colors))
palette = Image.new('P', (1, 1))
palette.putpalette([value for color in colors for value in color])
paletted = [im.quantize(palette=palette, dither=Image.Dither.NONE) for im in frames]
paletted[0].save(out/'scoped-memory-demo.gif',save_all=True,append_images=paletted[1:],
                duration=durations,loop=0,optimize=True,disposal=1)
print(json.dumps({'gif':str(out/'scoped-memory-demo.gif'),'seconds':sum(durations)/1000,
                  'frames':len(frames),'bytes':(out/'scoped-memory-demo.gif').stat().st_size}))
