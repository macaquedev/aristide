import { useEffect, useState } from 'react';
import { Button, Card, Drawer, Group, NumberInput, SegmentedControl, Select, Stack, Text } from '@mantine/core';
import { BuildStudy, buildLayouts } from './BuildStudy';
import { TuningStudy, tuningLayouts } from './TuningStudy';
import './study.css';

export function Study() {
  const params = new URLSearchParams(location.search);
  const [panel, setPanel] = useState(params.get('panel') === 'tuning' ? 'tuning' : 'build');
  const requested = params.get('layout');
  const [build, setBuild] = useState(buildLayouts.some(l => l.value === requested) ? requested! : 'rows');
  const [tuning, setTuning] = useState(tuningLayouts.some(l => l.value === requested) ? requested! : 'tree');
  const [density, setDensity] = useState('comfortable');
  const [assignment, setAssignment] = useState<string>();
  const [kind, setKind] = useState('MIDI');
  const [assigned, setAssigned] = useState<Record<string, string>>({});
  useEffect(() => {
    const query = new URLSearchParams({ study: '1', panel, layout: panel === 'build' ? build : tuning });
    history.replaceState(null, '', `?${query}`);
  }, [panel, build, tuning]);
  return <div className={`study ${density === 'compact' ? 'study-compact' : ''}`}><Stack>
    <Card withBorder><Group justify="space-between"><Stack gap={4}><Text fw={600}>Aristide · clickable design studies</Text><Text c="dimmed">Prototype data only. Changes are not saved and do not control audio.</Text></Stack><Button component="a" href="/" variant="default">Return to Play</Button></Group></Card>
    <Group justify="space-between"><SegmentedControl aria-label="Study panel" value={panel} onChange={setPanel} data={[{ value: 'build', label: 'Build' }, { value: 'tuning', label: 'Tuning' }]}/>
      <SegmentedControl aria-label="Density" value={density} onChange={setDensity} data={[{ value: 'comfortable', label: 'Comfortable' }, { value: 'compact', label: 'Compact' }]}/></Group>
    <div className="study-layouts"><SegmentedControl aria-label="Layout" value={panel === 'build' ? build : tuning} onChange={panel === 'build' ? setBuild : setTuning} data={panel === 'build' ? buildLayouts : tuningLayouts}/></div>
    <Text c="dimmed">{panel === 'build' ? 'Try adding a voice, changing its pitch and delay, choosing a source, and holding a key while editing.' : 'Try switching scopes, releasing inheritance on a division, changing its reference pitch, and editing Custom deviation bars.'} Numbers drag vertically, tap for a stepper, or hold for assignment.</Text>
    <div hidden={panel !== 'build'}><BuildStudy layout={build} compact={density === 'compact'} assign={setAssignment}/></div>
    <div hidden={panel !== 'tuning'}><TuningStudy layout={tuning} assign={setAssignment}/></div>
    {Object.entries(assigned).map(([address, control]) => <Text key={address} c="violet">Study assignment: {address} → {control}</Text>)}
    <Drawer opened={Boolean(assignment)} onClose={() => setAssignment(undefined)} title="Assign control · prototype" position="right" size="lg"><Stack><Text>{assignment}</Text>
      <SegmentedControl value={kind} onChange={setKind} data={['MIDI', 'LFO']}/>
      {kind === 'MIDI' ? <Text c="dimmed">The final sheet will learn a pedal, knob or button here. This study does not listen to hardware.</Text> : <><Select label="Waveform" defaultValue="Sine" data={['Sine', 'Triangle', 'Square']}/><NumberInput label="Rate (Hz)" defaultValue={.5} min={.01} max={20} step={.1}/><NumberInput label="Depth" defaultValue={10} min={0}/></>}
      <Button onClick={() => { if (assignment) setAssigned({ ...assigned, [assignment]: kind }); setAssignment(undefined); }}>Preview assignment</Button>
    </Stack></Drawer>
  </Stack></div>;
}
