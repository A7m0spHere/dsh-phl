"""Save/enrichment race regression, with delayed IPC and no real keys or instances.

Start `npm run dev`, then run:
  python scripts/model-enrichment-smoke.py --url http://localhost:5180
Requires Python Playwright and Chrome. The temporary Vite page is removed on exit.
"""
import argparse
import json
import tempfile
from pathlib import Path

from playwright.sync_api import expect, sync_playwright


HTML = r'''<!doctype html><html><head><meta charset="utf-8"></head>
<body><div id="root"></div>
<script>
window.pending=[];
window.saved=[];
window.__TAURI_INTERNALS__={invoke:async(cmd,args)=>{
  if(cmd==='fetch_provider_models') return [{id:'gpt-test',name:'Endpoint name'}];
  if(cmd==='plugin:model-metadata|enrich_model_metadata') {
    await new Promise((resolve,reject)=>window.pending.push({resolve,reject}));
    return {catalogStatus:'fresh',results:args.models.map(model=>({
      model:{...model,contextWindow:model.contextWindow??200000,maxTokens:model.maxTokens??32768},
      matched:true,changed:true,ambiguous:false
    }))};
  }
  throw new Error('Unexpected IPC: '+cmd);
}};
</script>
<script type="module">
import React from 'react';
import ReactDOM from 'react-dom/client';
import {ProviderCard} from '/src/features/api-config/ProviderCard.tsx';
import {NewProviderCard} from '/src/features/api-config/fields.tsx';
import {useApiConfigStore} from '/src/stores/apiConfigStore.ts';
import {useUIStore} from '/src/stores/uiStore.ts';
import '/src/index.css';
useUIStore.getState().setMotion('off');
useApiConfigStore.setState({config:null,saving:false,addProvider:async draft=>{
  window.saved.push(draft);return {...draft,id:'created'};
}});
function Harness(){
  const [visible,setVisible]=React.useState(true);
  const [provider,setProvider]=React.useState({id:'test-provider',name:'openai',kind:'custom',
    enabled:true,apiKeyEnv:'TEST_KEY',baseURL:'https://example.invalid/v1',models:[]});
  const create=new URLSearchParams(location.search).has('new');
  return React.createElement('main',{className:'p-5'},create
    ? visible && React.createElement(NewProviderCard,{onCancel:()=>setVisible(false)})
    : React.createElement(ProviderCard,{provider,isDefault:false,usedBy:0,onDelete:()=>{},
        onEdit:patch=>{window.saved.push(patch);setProvider({...provider,...patch})}}));
}
ReactDOM.createRoot(document.getElementById('root')).render(React.createElement(Harness));
</script></body></html>'''


def open_form(page, url, create=False):
    page.goto(url + ('?new' if create else ''))
    page.wait_for_load_state('networkidle')
    if create:
        page.get_by_label('名称', exact=True).fill('test provider')
        page.get_by_label('Base URL', exact=False).fill('https://example.invalid/v1')
    else:
        page.get_by_role('button', name='编辑', exact=True).click()


def start_add(page):
    page.get_by_role('button', name='获取可用模型', exact=True).click()
    page.get_by_role('checkbox').first.check()
    page.get_by_role('button', name='添加所选（1）', exact=True).click()
    page.wait_for_function('window.pending.length === 1')
    expect(page.get_by_role('button', name='模型补全中…', exact=True)).to_be_disabled()
    # Even a synthetic click must not trigger the owner's save handler.
    page.get_by_role('button', name='模型补全中…', exact=True).dispatch_event('click')
    assert page.evaluate('window.saved.length') == 0


def release(page, fail=False):
    page.evaluate('''fail=>{const task=window.pending.shift();
      if(fail)task.reject(new Error('metadata unavailable'));else task.resolve();}''', fail)


def run(url):
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True, channel='chrome')
        try:
            page = browser.new_page(viewport={'width': 1000, 'height': 900})
            errors = []
            page.on('pageerror', lambda error: errors.append(str(error)))
            # Both forms must preserve selected IDs on success AND degraded IPC.
            for create in (False, True):
                for fail in (False, True):
                    open_form(page, url, create)
                    start_add(page)
                    release(page, fail)
                    save = page.get_by_role('button', name='保存到全局库' if create else '保存', exact=True)
                    expect(save).to_be_enabled()
                    save.click()
                    page.wait_for_function('window.saved.length === 1')
                    models = page.evaluate('window.saved[0].models')
                    assert len(models) == 1 and models[0]['id'] == 'gpt-test'
                    assert models[0]['name'] == 'Endpoint name'
                    assert (models[0].get('maxTokens') == 32768) is (not fail)

            # Manual single-row and batch fill use the same save barrier.
            for action in ('补全信息', '补全模型信息'):
                open_form(page, url)
                page.get_by_role('button', name='添加', exact=True).click()
                page.get_by_label('模型 ID').fill('manual-test')
                page.get_by_text('编辑详细字段', exact=True).click()
                page.get_by_label('上下文窗口', exact=True).fill('999')
                page.get_by_role('button', name=action, exact=True).click()
                expect(page.get_by_role('button', name='模型补全中…', exact=True)).to_be_disabled()
                release(page)
                page.get_by_role('button', name='保存', exact=True).click()
                assert page.evaluate('window.saved[0].models[0].contextWindow') == 999
                assert page.evaluate('window.saved[0].models[0].maxTokens') == 32768

            # Cancelling and reopening must release the barrier; late responses stay discarded.
            open_form(page, url)
            start_add(page)
            page.get_by_role('button', name='取消', exact=True).filter(visible=True).first.click()
            expect(page.locator('fieldset')).to_have_count(0)
            page.get_by_role('button', name='编辑', exact=True).click()
            expect(page.get_by_role('button', name='保存', exact=True)).to_be_enabled()
            release(page)
            page.get_by_role('button', name='保存', exact=True).click()
            assert page.evaluate('window.saved[0].models') == []
            assert not errors, errors
            print(json.dumps({'passed': 7, 'pageErrors': errors}))
        finally:
            browser.close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url', default='http://localhost:5180')
    args = parser.parse_args()
    scratch = Path(__file__).resolve().parents[1] / '.scratch'
    scratch.mkdir(exist_ok=True)
    with tempfile.NamedTemporaryFile(mode='w', suffix='.html', prefix='enrichment-save-',
                                     dir=scratch, encoding='utf-8', delete=False) as handle:
        handle.write(HTML)
        fixture = Path(handle.name)
    try:
        run(f'{args.url.rstrip("/")}/.scratch/{fixture.name}')
    finally:
        fixture.unlink(missing_ok=True)
