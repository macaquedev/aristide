/** Small diagrams of the actual arrangements, used as layout navigation. */
export function LayoutPreview({ layout }: { layout: string }) {
  const box = (x: number, y: number, w: number, h: number, active = false) =>
    <rect key={`${x}-${y}`} x={x} y={y} width={w} height={h} rx="1" fill="currentColor" opacity={active ? .9 : .25}/>;
  const rows = [8, 24, 40];
  return <svg viewBox="0 0 160 56" aria-hidden="true">
    {layout === 'rows' && rows.flatMap(y => [box(4, y, 36, 10), ...[48, 76, 104, 132].map(x => box(x, y, 24, 10, x === 76))])}
    {layout === 'steps' && rows.flatMap((y, i) => [box(4, y, 20, 10), ...[32, 58, 84, 110, 136].map((x, j) => box(x, y, 20, 10, i === j))])}
    {layout === 'roll' && <>{[8, 20, 32, 44].map(y => <path key={y} d={`M4 ${y} H156`} stroke="currentColor" opacity=".2"/>)}{box(12, 36, 36, 7, true)}{box(56, 24, 36, 7, true)}{box(100, 12, 36, 7, true)}</>}
    {layout === 'keys' && <>{Array.from({length: 12}, (_, i) => box(4 + i * 13, 4, 10, 17, i === 4))}{box(4, 28, 43, 24)}{box(54, 28, 102, 24, true)}</>}
    {layout === 'tree' && <>{rows.map((y, i) => box(4 + i * 8, y, 38 - i * 8, 10))}{box(54, 8, 102, 22, true)}{box(54, 36, 102, 14)}</>}
    {layout === 'table' && rows.flatMap(y => [4, 44, 84, 124].map(x => box(x, y, 32, 10, x === 84)))}
    {layout === 'cascade' && [4, 44, 84, 124].map((x, i) => box(x, 5 + i * 8, 32, 22, i === 2))}
    {layout === 'keyboard' && <>{[12, 30, 48, 66, 84, 102, 120, 138].map((x, i) => box(x, 28 - (i % 3) * 7, 10, 8 + (i % 3) * 7, true))}{box(4, 42, 152, 10)}</>}
  </svg>;
}
