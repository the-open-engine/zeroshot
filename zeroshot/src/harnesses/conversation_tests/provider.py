#!/usr/bin/env python3
import json, os, pathlib, re, sys
harness = 'HARNESS_NAME'
args = sys.argv[1:]
answer = json.dumps({'response': None})
expected = pathlib.Path('expected-id')
content = 'first-turn-content-only-in-provider-files'

def emit(value):
    print(json.dumps(value), flush=True)

def option(name, default=None):
    return args[args.index(name)+1] if name in args else default

def address(identifier, path, resumed):
    path = pathlib.Path(path)
    if expected.exists():
        assert resumed and identifier == expected.read_text(), (identifier, resumed)
        assert path.read_text() == content, path
    else:
        assert not resumed
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
    pathlib.Path('address.json').write_text(json.dumps({'id':identifier, 'content':path.read_text(), 'path':str(path)}))

if harness == 'codex':
    if not args or args[0] != 'exec':
        emit({})
        sys.exit(0)
    sys.stdin.read()
    identifier = 'checkpoint-thread'
    resumed = 'resume' in args
    address(identifier, pathlib.Path(os.environ['CODEX_HOME'])/'sessions/2026/11/14'/f'rollout-{identifier}.jsonl', resumed)
    emit({'type':'thread.started','thread_id':identifier})
    emit({'type':'item.completed','item':{'type':'agent_message','text':answer}})
    completed = {'type':'turn.completed'}
    if not pathlib.Path('omit-usage').exists():
        completed['usage'] = {'input_tokens':130 if resumed else 100,'output_tokens':17 if resumed else 10}
    emit(completed)
elif harness == 'claude':
    if args == ['--version']:
        print('2.1.318 (Claude Code)')
        sys.exit(0)
    if '--safe-mode' in args:
        for line in sys.stdin:
            request=json.loads(line)
            subtype=request.get('request',{}).get('subtype')
            data={'effective':{},'sources':[]} if subtype=='get_settings' else {'current_permission_mode':'default'}
            emit({'type':'control_response','response':{'subtype':'success','request_id':request['request_id'],'response':data}})
        sys.exit(0)
    sys.stdin.read()
    identifier=option('--resume','checkpoint-session')
    home=pathlib.Path(os.environ.get('CLAUDE_CONFIG_DIR',str(pathlib.Path(os.environ['HOME'])/'.claude')))
    project=re.sub('[^a-zA-Z0-9]','-',os.getcwd())
    address(identifier,home/'projects'/project/f'{identifier}.jsonl','--resume' in args)
    emit({'type':'system','subtype':'init','session_id':identifier})
    emit({'type':'result','subtype':'success','is_error':False,'result':answer,'session_id':identifier})
elif harness == 'pi':
    sys.stdin.read()
    identifier=option('--session-id')
    address(identifier,pathlib.Path(option('--session-dir'))/f'2026-11-14_{identifier}.jsonl',expected.exists())
    emit({'type':'session','id':identifier})
    emit({'type':'message_end','message':{'role':'assistant','content':[{'type':'text','text':answer}],'stopReason':'stop','usage':{'input':10,'output':2}}})
elif harness == 'opencode':
    identifier=option('--session','ses_checkpoint')
    address(identifier,pathlib.Path(os.environ['HOME'])/'.local/share/opencode/opencode.db','--session' in args)
    emit({'type':'text','sessionID':identifier,'part':{'id':'text','messageID':'message','text':answer}})
    emit({'type':'step_finish','sessionID':identifier,'part':{'id':'finish','messageID':'message','reason':'stop','tokens':{'input':10,'output':2,'reasoning':0,'cache':{'read':0,'write':0}}}})
elif harness == 'hermes':
    emit({'type':'ready','protocol':1})
    init=json.loads(sys.stdin.readline())
    address(init['sessionId'],init['dbPath'],expected.exists())
    emit({'type':'initialized','sessionId':init['sessionId']})
    for line in sys.stdin:
        message=json.loads(line)
        if message['type']=='stop':
            emit({'type':'closed','clean':True})
            break
        emit({'type':'result','message':answer,'completed':True,'failed':False,'interrupted':False,'usage':{'input':10,'output':2}})
elif harness == 'copilot':
    def read():
        length=None
        while True:
            line=sys.stdin.buffer.readline()
            if not line:return None
            if line==b'\r\n':break
            key,value=line.decode().split(':',1)
            if key.lower()=='content-length':length=int(value)
        return json.loads(sys.stdin.buffer.read(length))
    def send(value):
        body=json.dumps({'jsonrpc':'2.0',**value}).encode()
        sys.stdout.buffer.write(f'Content-Length: {len(body)}\r\n\r\n'.encode()+body)
        sys.stdout.buffer.flush()
    while (message:=read()) is not None:
        method=message.get('method')
        params=message.get('params',{})
        if method=='connect':result={'protocolVersion':3}
        elif method in ('session.create','session.resume'):
            identifier=params['sessionId']
            address(identifier,pathlib.Path(os.environ['COPILOT_HOME'])/'session-state'/identifier/'events.jsonl',method=='session.resume')
            result={'sessionId':identifier}
        elif method=='session.send':
            send({'method':'session.event','params':{'sessionId':identifier,'event':{'type':'assistant.message','data':{'content':answer}}}})
            result={'messageId':'message'}
        elif method=='session.detach':result={'success':True}
        else:result={}
        if 'id' in message:send({'id':message['id'],'result':result})
