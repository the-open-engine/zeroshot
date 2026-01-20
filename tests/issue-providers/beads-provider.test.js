/**
 * Tests for Beads issue provider
 */

const { expect } = require('chai');
const BeadsProvider = require('../../src/issue-providers/beads-provider');

describe('BeadsProvider', () => {
  describe('detectIdentifier', () => {
    const settings = {};
    const gitContext = null;

    it('detects standard bd-xxx format', () => {
      expect(BeadsProvider.detectIdentifier('bd-abc123', settings, gitContext)).to.equal(true);
      expect(BeadsProvider.detectIdentifier('bd-a1b2c3', settings, gitContext)).to.equal(true);
      expect(BeadsProvider.detectIdentifier('bd-ABC123', settings, gitContext)).to.equal(true);
    });

    it('detects explicit beads: prefix', () => {
      expect(BeadsProvider.detectIdentifier('beads:test-123', settings, gitContext)).to.equal(true);
      expect(BeadsProvider.detectIdentifier('beads:bd-abc', settings, gitContext)).to.equal(true);
      expect(BeadsProvider.detectIdentifier('beads:anything', settings, gitContext)).to.equal(true);
    });

    it('detects beads:ready selector', () => {
      expect(BeadsProvider.detectIdentifier('beads:ready', settings, gitContext)).to.equal(true);
      expect(BeadsProvider.detectIdentifier('beads:ready:P0', settings, gitContext)).to.equal(true);
      expect(BeadsProvider.detectIdentifier('beads:ready:P1', settings, gitContext)).to.equal(true);
    });

    it('does NOT detect bare numbers', () => {
      expect(BeadsProvider.detectIdentifier('123', settings, gitContext)).to.equal(false);
      expect(BeadsProvider.detectIdentifier('456789', settings, gitContext)).to.equal(false);
    });

    it('does NOT detect GitHub URLs', () => {
      expect(
        BeadsProvider.detectIdentifier(
          'https://github.com/org/repo/issues/123',
          settings,
          gitContext
        )
      ).to.equal(false);
    });

    it('does NOT detect simple hyphenated words', () => {
      // These should fail because bd show would fail
      expect(BeadsProvider.detectIdentifier('fix-typo', settings, gitContext)).to.equal(false);
      expect(BeadsProvider.detectIdentifier('my-branch', settings, gitContext)).to.equal(false);
    });
  });

  describe('getRequiredTool', () => {
    it('returns bd CLI info', () => {
      const tool = BeadsProvider.getRequiredTool();
      expect(tool.name).to.equal('bd');
      expect(tool.checkCmd).to.equal('bd --version');
      expect(tool.installHint).to.include('beads');
    });
  });

  describe('supportsPR', () => {
    it('returns false (beads is not a git host)', () => {
      expect(BeadsProvider.supportsPR()).to.equal(false);
    });
  });

  describe('getSettingsSchema', () => {
    it('returns empty schema (no settings needed)', () => {
      const schema = BeadsProvider.getSettingsSchema();
      expect(schema).to.deep.equal({});
    });
  });

  describe('_extractAcceptanceCriteria', () => {
    const provider = new BeadsProvider();

    it('extracts AC section from description', () => {
      const description = `
Some intro text.

Acceptance Criteria:
- Must do X
- Must do Y
- Must pass tests

## Next Section
More stuff
`;
      const ac = provider._extractAcceptanceCriteria(description);
      expect(ac).to.include('Must do X');
      expect(ac).to.include('Must do Y');
      expect(ac).to.not.include('Next Section');
    });

    it('returns null for description without AC', () => {
      const description = 'Just a simple description without criteria';
      const ac = provider._extractAcceptanceCriteria(description);
      expect(ac).to.be.null;
    });
  });

  describe('_resolveIssueId', () => {
    const provider = new BeadsProvider();

    it('removes beads: prefix for direct IDs', () => {
      expect(provider._resolveIssueId('beads:test-123')).to.equal('test-123');
      expect(provider._resolveIssueId('beads:bd-abc')).to.equal('bd-abc');
    });

    it('passes through other formats unchanged', () => {
      expect(provider._resolveIssueId('bd-abc123')).to.equal('bd-abc123');
      expect(provider._resolveIssueId('AppKiln-xyz')).to.equal('AppKiln-xyz');
    });

    // Note: beads:ready tests would require mocking bd CLI
  });

  describe('_escapeShellArg', () => {
    const provider = new BeadsProvider();

    it('escapes double quotes', () => {
      expect(provider._escapeShellArg('hello "world"')).to.equal('hello \\"world\\"');
    });

    it('escapes newlines', () => {
      expect(provider._escapeShellArg('line1\nline2')).to.equal('line1\\nline2');
    });

    it('handles combined escaping', () => {
      expect(provider._escapeShellArg('say "hello"\nworld')).to.equal('say \\"hello\\"\\nworld');
    });
  });

  describe('lifecycle hooks interface', () => {
    const provider = new BeadsProvider();

    it('has onClusterComplete method', () => {
      expect(typeof provider.onClusterComplete).to.equal('function');
    });

    it('has onClusterFailed method', () => {
      expect(typeof provider.onClusterFailed).to.equal('function');
    });

    it('onClusterComplete returns promise', async () => {
      // Should not throw with minimal inputData
      const result = provider.onClusterComplete({}, { clusterId: 'test' }, {});
      expect(result).to.be.instanceOf(Promise);
      await result; // Should resolve without error
    });

    it('onClusterFailed returns promise', async () => {
      // Should not throw with minimal inputData
      const result = provider.onClusterFailed({}, { clusterId: 'test', reason: 'test' }, {});
      expect(result).to.be.instanceOf(Promise);
      await result; // Should resolve without error
    });
  });

  describe('_parseIssue', () => {
    const provider = new BeadsProvider();

    it('parses basic issue correctly', () => {
      const issue = {
        id: 'AppKiln-123',
        title: 'Test Issue',
        description: 'Test description',
        status: 'open',
        priority: 1,
        issue_type: 'feature',
      };

      const result = provider._parseIssue(issue);

      expect(result.beadsId).to.equal('AppKiln-123');
      expect(result.title).to.equal('Test Issue');
      expect(result.body).to.equal('Test description');
      expect(result.beadsStatus).to.equal('open');
      expect(result.beadsPriority).to.equal(1);
      expect(result.beadsType).to.equal('feature');
      expect(result.number).to.be.null; // Beads uses string IDs
      expect(result.url).to.be.null; // Beads is local
    });

    it('includes dependencies in context', () => {
      const issue = {
        id: 'AppKiln-123',
        title: 'Test Issue',
        description: 'Test',
        status: 'open',
        priority: 1,
        dependencies: [
          { id: 'AppKiln-100', title: 'Blocker 1', status: 'closed' },
          { id: 'AppKiln-101', title: 'Blocker 2', status: 'open' },
        ],
      };

      const result = provider._parseIssue(issue);

      expect(result.context).to.include('Dependencies');
      expect(result.context).to.include('AppKiln-100');
      expect(result.context).to.include('AppKiln-101');
      expect(result.context).to.include('✅'); // Closed blocker
      expect(result.context).to.include('⏳'); // Open blocker
      expect(result.context).to.include('Warning'); // Has open blockers
      expect(result.beadsDependencies).to.have.lengthOf(2);
    });

    it('extracts acceptance criteria from description', () => {
      const issue = {
        id: 'AppKiln-123',
        title: 'Test',
        description: `
Do something.

Acceptance Criteria:
- Criterion 1
- Criterion 2
`,
        status: 'open',
        priority: 1,
      };

      const result = provider._parseIssue(issue);

      expect(result.context).to.include('Acceptance Criteria (Extracted)');
      expect(result.context).to.include('Criterion 1');
    });
  });
});
