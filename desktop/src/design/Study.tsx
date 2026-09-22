import { useEffect, useState } from 'react';
import { Badge, Button, Drawer, Group, NumberInput, SegmentedControl, Select, Stack, Text } from '@mantine/core';
import { BuildStudy, buildLayouts } from './BuildStudy';
import { PianoRollStudy, rollLayouts } from './PianoRollStudy';
import { TuningStudy, tuningLayouts } from './TuningStudy';
import { TuningDeskStudy, tuningDeskLayouts } from './TuningDeskStudy';
import { ArrowLeft, VolumeOff } from 'lucide-react';
import { LayoutPreview } from './LayoutPreview';
import './study.css';

export function Study() {
  const params = new URLSearchParams(location.search);
  const [panel, setPanel] = useState(params.get('panel') === 'tuning' ? 'tuning' : 'build');
  const requested = params.get('layout');
  const [legacy] = useState(buildLayouts.some(l => l.value === requested) && (requested !== 'roll' || params.get('legacy') === '1'));
  const choices = legacy ? buildLayouts : rollLayouts;
  const [build, setBuild] = useState(choices.some(l => l.value === requested) ? requested! : 'split');
  const [legacyTuning] = useState(params.get('legacy') === '1' && tuningLayouts.some(l => l.value === requested));
  const tuningChoices = legacyTuning ? tuningLayouts : tuningDeskLayouts;
  const [tuning, setTuning] = useState(tuningChoices.some(l => l.value === requested) ? requested! : 'channel');
  const [density, setDensity] = useState('comfortable');
  const [assignment, setAssignment] = useState<string>();
  const [kind, setKind] = useState('MIDI');
  const [assigned, setAssigned] = useState<Record<string, string>>({});
  useEffect(() => {
    const query = new URLSearchParams({ study: '1', panel, layout: panel === 'build' ? build : tuning });
    if (panel === 'build' ? legacy : legacyTuning) query.set('legacy', '1');
    history.replaceState(null, '', `?${query}`);
  }, [panel, build, tuning, legacy, legacyTuning]);
  return <div className={`study ${panel === 'tuning' && !legacyTuning ? 'tuning-study-shell' : ''} ${density === 'compact' ? 'study-compact' : ''}`}><Stack>
    <header className="study-header"><Group><Button component="a" href="/" variant="default" leftSection={<ArrowLeft size={16}/>}>Play</Button><Text fw={600}>Design studies</Text></Group><Group gap="xs"><Badge color="gray" variant="outline">Prototype</Badge><Text c="dimmed"><VolumeOff size={14} aria-hidden="true"/> No audio</Text><Text c="dimmed">Not saved</Text></Group></header>
    <Group justify="space-between"><SegmentedControl aria-label="Study panel" value={panel} onChange={setPanel} data={[{ value: 'build', label: 'Build' }, { value: 'tuning', label: 'Tuning' }]}/>
      <SegmentedControl aria-label="Density" value={density} onChange={setDensity} data={[{ value: 'comfortable', label: 'Comfortable' }, { value: 'compact', label: 'Compact' }]}/></Group>
    <div className="study-layouts" role="group" aria-label="Layout">{(panel === 'build' ? choices : tuningChoices).map(layout => <Button key={layout.value} className="layout-choice" variant={(panel === 'build' ? build : tuning) === layout.value ? 'filled' : 'default'} aria-pressed={(panel === 'build' ? build : tuning) === layout.value} onClick={() => (panel === 'build' ? setBuild : setTuning)(layout.value)}><LayoutPreview layout={layout.value}/><span>{layout.label.replace(/^\d · /, '')}</span></Button>)}</div>
    <div hidden={panel !== 'build'}>{legacy ? <BuildStudy layout={build} compact={density === 'compact'} assign={setAssignment}/> : <PianoRollStudy layout={build} compact={density === 'compact'} assign={setAssignment}/>}</div>
    <div hidden={panel !== 'tuning'}>{legacyTuning ? <TuningStudy layout={tuning} assign={setAssignment}/> : <TuningDeskStudy layout={tuning} assign={setAssignment}/>}</div>
    {Object.entries(assigned).map(([address, control]) => <Group key={address}><Badge color="violet" variant="light">{control}</Badge><Text>{parameterLabel(address)}</Text></Group>)}
    <Drawer opened={Boolean(assignment)} onClose={() => setAssignment(undefined)} title="Assign control · prototype" position="right" size="lg"><Stack><Text fw={600}>{parameterLabel(assignment ?? '')}</Text>
      <SegmentedControl value={kind} onChange={setKind} data={['MIDI', 'LFO']}/>
      {kind === 'MIDI' ? <Text c="dimmed">MIDI unavailable in prototype</Text> : <><Select label="Waveform" defaultValue="Sine" data={['Sine', 'Triangle', 'Square']}/><NumberInput label="Rate (Hz)" defaultValue={.5} min={.01} max={20} step={.1}/><NumberInput label="Depth" defaultValue={10} min={0}/></>}
      <Button onClick={() => { if (assignment) setAssigned({ ...assigned, [assignment]: kind }); setAssignment(undefined); }}>Preview assignment</Button>
    </Stack></Drawer>
  </Stack></div>;
}

function parameterLabel(address: string) {
  const parts = address.split('/');
  const field = parts[0] === 'tuning' ? parts[2] : parts[4];
  return ({ deviation: 'Note deviation', hz: 'Reference pitch', fine: 'Fine offset', steps: 'Step count', period: 'Repeat interval', interval: 'Step interval', referenceKey: 'Reference key', pitch: 'Pitch', delay: 'Delay', level: 'Level', low: 'Low key', high: 'High key' } as Record<string, string>)[field] ?? field;
}
