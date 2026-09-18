import { useEffect, useRef } from 'react';
import { ArrowLeft } from 'lucide-react';
import { ThemeControl } from './ThemeControl';
import './defaults.css';

export function DefaultsPage({ back }: { back: () => void }) {
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    heading.current?.focus();
  }, []);
  return (
    <div className="defaults-page">
      <header className="defaults-menu">
        <button className="text-button" onClick={back}>
          <ArrowLeft size={17} /> Back to editor
        </button>
        <ThemeControl />
      </header>
      <main className="defaults-content">
        <h1 ref={heading} tabIndex={-1}>
          How workflows behave
        </h1>
        <p className="defaults-intro">
          Choose the work and where its inputs come from. The editor handles the wiring.
        </p>
        <dl>
          <div>
            <dt>Inputs & outputs</dt>
            <dd>
              Select a run input or an earlier result. Its type and route are filled in
              automatically. Each result belongs to its producing node.
            </dd>
          </div>
          <div>
            <dt>Repeat until approved</dt>
            <dd>
              Work, review, revise. Review feedback goes into the next attempt. Approval ends the
              loop; reaching the attempt limit fails it. Work afterward uses the final round’s
              results.
            </dd>
          </div>
          <div>
            <dt>Repeat a set number of times</dt>
            <dd>
              Complete every round, then continue with the final results. Earlier rounds remain in
              run history.
            </dd>
          </div>
          <div>
            <dt>For each item</dt>
            <dd>
              Process the list and collect results in input order. An empty list returns an empty
              list. A failed item fails the operation.
            </dd>
          </div>
          <div>
            <dt>Parallel work</dt>
            <dd>
              Wait for every branch. All branch results are available afterward, under their own
              names.
            </dd>
          </div>
          <div>
            <dt>Decisions</dt>
            <dd>
              Take the first matching path. Otherwise takes the remaining cases. A shared result
              must have the same type on every continuing path.
            </dd>
          </div>
          <div>
            <dt>Errors</dt>
            <dd>
              Execution errors stop the workflow. Required results must exist before they are used;
              missing results are never replaced with an older value.
            </dd>
          </div>
          <div>
            <dt>Files</dt>
            <dd>
              Agents write plans, code, and documents to files; Verifiers review them. Use small
              outputs for paths, decisions, and values needed by later work.
            </dd>
          </div>
        </dl>
        <p className="defaults-note">
          These rules describe workflows built with the visual editor. Graph JSON retains the full
          graph language. Existing custom behavior is preserved.
        </p>
      </main>
    </div>
  );
}
