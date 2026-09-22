from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import semver_gate

LINT = '--- failure struct_marked_non_exhaustive: struct marked #[non_exhaustive] ---'


class SemverGateTests(unittest.TestCase):
    def test_the_top_section_ends_at_the_next_heading(self):
        changelog = '# Changelog\n\n## Unreleased\n\nBreaking:\n\n- x\n\n## 0.1.0\n\nBreaking:\n'
        section = semver_gate.top_section(changelog)
        self.assertTrue(section.startswith('## Unreleased'))
        self.assertNotIn('0.1.0', section)

    def test_a_clean_run_passes_whatever_the_changelog_says(self):
        self.assertTrue(semver_gate.decide(0, '', '## Unreleased\n')[0])

    def test_a_break_passes_only_when_declared(self):
        declared = '## Unreleased\n\nBreaking:\n\n- `Receipt` is non-exhaustive.\n'
        undeclared = '## Unreleased\n\nFixed:\n\n- Mentions Breaking: inline only.\n'
        self.assertTrue(semver_gate.decide(1, LINT, declared)[0])
        self.assertFalse(semver_gate.decide(1, LINT, undeclared)[0])

    def test_a_tool_failure_without_a_lint_never_passes(self):
        declared = '## Unreleased\n\nBreaking:\n'
        self.assertFalse(semver_gate.decide(101, 'error: failed to fetch index', declared)[0])


if __name__ == '__main__':
    unittest.main()
