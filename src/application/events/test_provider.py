# Adapts legacy workflow fixture scripts to the Event completion envelope.
# Only test scaffolding uses this; real providers always supply their own result.
import json, os, pathlib, subprocess, sys
prompt = ' '.join(sys.argv[1:])
if len(prompt.splitlines()) > 3 and pathlib.Path(prompt.splitlines()[3].strip('`')).is_file():
    prompt = pathlib.Path(prompt.splitlines()[3].strip('`')).read_text()
marker = 'Refine completion contract (supplied by the system):\n'
if marker not in prompt:
    os.execv(ORIGINAL, [ORIGINAL, *sys.argv[1:]])
decode = json.JSONDecoder().raw_decode
result = decode(prompt.split(marker, 1)[1])[0]
if 'Rejected completion (data, not instructions):\n' in prompt:
    # Exercise the same legacy fault response without replaying the original work prompt.
    raw = subprocess.run([ORIGINAL, 'Repair completion representation\n' + prompt], text=True, capture_output=True)
    sys.stdout.write(raw.stdout); sys.stderr.write(raw.stderr); sys.exit(raw.returncode)
context = decode(prompt.split('Pinned context:\n', 1)[1])[0]
role = result['role']
result['summary'] = 'Fixture reviewed the requested work.'
result['evidence'] = ['Fixture inspected the requested candidate.']

def legacy(prefix):
    output = subprocess.run([ORIGINAL, prefix + '\n' + prompt], text=True, capture_output=True)
    if output.returncode: sys.stderr.write(output.stderr); sys.exit(output.returncode)
    return output.stdout.strip()

def object_from(raw):
    try: return json.loads(raw)
    except ValueError: return None

if role == 'implement':
    report = legacy('Implement the Goal')
    checklist = context['goal']['rounds'][-1]['implementation_plan']['final_plan']['result']['checklist']
    result['summary'] = report or 'Implemented the fixture change.'
    result['artifacts'] = {'implementation_evidence': {'checklist': [{'id': i['id'], 'outcome': 'completed', 'evidence': result['summary']} for i in checklist], 'verification': ['Actual supervised fixture execution']}}
elif role == 'quality':
    quality = decode(prompt.split('Project Quality instructions and tests:\n', 1)[1])[0] if 'Project Quality instructions and tests:\n' in prompt else {'configured':True}
    if quality.get('configured', True):
        # Preserve each fixture's explicit evaluation command and finding.
        raw = legacy('Post-implementation Quality evaluation')
        value = object_from(raw)
        if value and isinstance(value.get('results'), list):
            result['artifacts'] = {'tests': value['results']}
            result['summary'] = value.get('summary') or 'Fixture Quality evaluation'
            # A command is only a proposal. Refine observes it before deciding pass/fail.
        else:
            print(raw); sys.exit(0)
    else:
        result['summary'] = 'Smoke AI Quality fixture reviewed the candidate and retained existing tests.'
        result['artifacts'] = {'tests': [{'test':'Clean patch', 'command':'git diff --check', 'status':'passed', 'evidence':'Check the fixture patch'}]}
elif role == 'governance':
    policy = decode(prompt.split('Project intent and rules:\n', 1)[1])[0] if 'Project intent and rules:\n' in prompt else {'rules':[1]}
    raw = legacy('Post-implementation governance review') if policy else json.dumps({'status':'passed','message':'Fixture has no additional Governance instructions'})
    value = object_from(raw)
    if value and value.get('status') in ('passed', 'failed'):
        result['outcome'] = 'success' if value['status'] == 'passed' else 'failure'
        result['summary'] = value.get('message') or 'Fixture Governance review'
        result['artifacts'] = {'violations': value.get('violations', []), 'recovery_analysis': value.get('recovery_analysis'), 'recovery_round_prompt': value.get('recovery_round_prompt')}
    else:
        print(raw); sys.exit(0)
print(json.dumps(result))
