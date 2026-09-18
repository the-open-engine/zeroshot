import { useMemo, useState } from 'react';
import { humanize } from './run-history';
import {
  decodeRecordedValue,
  isRecordedObject,
  recordedFieldPath,
  recordedProperty,
  recordedTextRange,
  RECORDED_PAGE_SIZE,
  RECORDED_TEXT_PAGE_SIZE,
  UNREADABLE_VALUE,
} from './recorded-value';

const MAX_DISPLAY_DEPTH = 32;
type ValueProps = { value: unknown; ancestors: readonly object[]; depth: number };

/** A bounded, read-only presentation of recorded data. Strings are always rendered as text. */
export function RecordedValue({ value }: { value: unknown }) {
  return (
    <div className="recorded-value">
      <Value value={value} ancestors={[]} depth={0} />
    </div>
  );
}

function Value({ value: rawValue, ancestors, depth }: ValueProps) {
  const value = useMemo(() => decodeRecordedValue(rawValue), [rawValue]);
  if (value === null) return <code className="recorded-scalar recorded-empty">null</code>;
  if (value === undefined) return <span className="recorded-empty">Not recorded</span>;
  if (value === UNREADABLE_VALUE)
    return <span className="recorded-limit">Unavailable property</span>;
  if (typeof value === 'string')
    return value ? (
      <Text value={value} />
    ) : (
      <code className="recorded-scalar recorded-empty">&quot;&quot;</code>
    );
  if (typeof value === 'boolean' || typeof value === 'number')
    return <code className="recorded-scalar">{Object.is(value, -0) ? '-0' : String(value)}</code>;
  if (!Array.isArray(value) && !isRecordedObject(value))
    return <span className="recorded-limit">Unsupported value</span>;
  if (ancestors.includes(value)) return <span className="recorded-limit">Circular reference</span>;
  if (depth >= MAX_DISPLAY_DEPTH)
    return <span className="recorded-limit">Display depth limit reached</span>;
  const count = Array.isArray(value) ? value.length : Object.keys(value).length;
  if (!count)
    return (
      <code className="recorded-scalar recorded-empty">{Array.isArray(value) ? '[]' : '{}'}</code>
    );
  if (depth >= 2 && (Array.isArray(value) || count > 1))
    return <Group value={value} ancestors={ancestors} depth={depth} count={count} />;
  return <Collection value={value} ancestors={ancestors} depth={depth} />;
}

function Group({ value, ancestors, depth, count }: ValueProps & { value: object; count: number }) {
  const [open, setOpen] = useState(false);
  const noun = Array.isArray(value)
    ? count === 1
      ? 'item'
      : 'items'
    : count === 1
      ? 'field'
      : 'fields';
  return (
    <details className="recorded-group" onToggle={(event) => setOpen(event.currentTarget.open)}>
      <summary>
        {count} {noun}
      </summary>
      {open && <Collection value={value} ancestors={ancestors} depth={depth} />}
    </details>
  );
}

function Collection({ value, ancestors, depth }: ValueProps & { value: object }) {
  const [page, setPage] = useState(0);
  const array = Array.isArray(value);
  const keys = useMemo(() => (array ? undefined : Object.keys(value)), [value, array]);
  const count = array ? value.length : keys!.length;
  const lastPage = Math.max(0, Math.ceil(count / RECORDED_PAGE_SIZE) - 1);
  const current = Math.min(page, lastPage);
  const start = current * RECORDED_PAGE_SIZE;
  const end = Math.min(start + RECORDED_PAGE_SIZE, count);
  const lineage = [...ancestors, value];
  const visible = Array.from({ length: end - start }, (_, index) => start + index);
  return (
    <>
      {array ? (
        <ol className="recorded-array" start={start + 1}>
          {visible.map((index) => (
            <li key={index}>
              <Value
                value={recordedProperty(value, String(index))}
                ancestors={lineage}
                depth={depth + 1}
              />
            </li>
          ))}
        </ol>
      ) : (
        <dl className="recorded-values">
          {visible.map((index) => {
            const key = keys![index];
            const field = recordedFieldPath(key, recordedProperty(value, key), lineage);
            return (
              <div className="recorded-field" key={key}>
                <dt className="recorded-path">
                  {field.path.map((segment, index) => (
                    <span key={index}>
                      {index > 0 && <span className="recorded-path-separator"> › </span>}
                      <span title={segment}>{segment ? humanize(segment) : '""'}</span>
                    </span>
                  ))}
                </dt>
                <dd>
                  <Value
                    value={field.value}
                    ancestors={field.ancestors}
                    depth={depth + field.path.length}
                  />
                </dd>
              </div>
            );
          })}
        </dl>
      )}
      {count > RECORDED_PAGE_SIZE && (
        <Pager
          start={start}
          end={end}
          count={count}
          current={current}
          last={lastPage}
          setPage={setPage}
          unit={array ? 'items' : 'fields'}
        />
      )}
    </>
  );
}

function Text({ value }: { value: string }) {
  const [page, setPage] = useState(0);
  const count = value.length;
  const { current, last: lastPage, start, end } = recordedTextRange(value, page);
  return (
    <>
      <div className="recorded-text">{value.slice(start, end)}</div>
      {count > RECORDED_TEXT_PAGE_SIZE && (
        <Pager
          start={start}
          end={end}
          count={count}
          current={current}
          last={lastPage}
          setPage={setPage}
          unit="characters"
        />
      )}
    </>
  );
}

function Pager({
  start,
  end,
  count,
  current,
  last,
  setPage,
  unit,
}: {
  start: number;
  end: number;
  count: number;
  current: number;
  last: number;
  setPage: (page: number) => void;
  unit: string;
}) {
  return (
    <nav className="recorded-pager" aria-label={`Recorded ${unit}`}>
      <button
        type="button"
        className="text-button"
        disabled={current === 0}
        onClick={() => setPage(current - 1)}
        aria-label={`Previous ${unit}`}
      >
        Previous
      </button>
      <span>
        {start + 1}–{end} of {count} {unit}
      </span>
      <button
        type="button"
        className="text-button"
        disabled={current === last}
        onClick={() => setPage(current + 1)}
        aria-label={`Next ${unit}`}
      >
        Next
      </button>
    </nav>
  );
}
