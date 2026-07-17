"""End-to-end tests for agent-chat, driven through the real CLI.

No aoe daemon needed: identities use --from, recipients use 'id:title', and
--no-doorbell skips `aoe send`. Run: python3 -m unittest discover -s tests
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
CLI = os.path.join(os.path.dirname(HERE), "agent-chat")

A = "a1:AgentA"
B = "b1:AgentB"

# A fake `aoe` on PATH so the commands that shell out (broadcast resolves via
# `aoe list`, identity via `aoe session current`, doorbell via `aoe send`) are
# testable without a real daemon. AGENT_CHAT_FAKE_MODE=nonjson makes `list`
# emit aoe's empty-profile plain-text notice instead of JSON.
FAKE_AOE = r'''#!/usr/bin/env python3
import json, os, sys
a = [x for x in sys.argv[1:] if x != "--json"]
if a[:1] == ["list"]:
    if os.environ.get("AGENT_CHAT_FAKE_MODE") == "nonjson":
        print("No sessions found in profile main."); sys.exit(0)
    print(json.dumps([{"id": "r1", "title": "Recruit", "group": "work/team"},
                      {"id": "r2", "title": "Scout", "group": "work/team"}]))
elif a[:2] == ["session", "current"]:
    print(json.dumps({"id": "caller", "title": "Caller"}))
sys.exit(0)
'''


class AgentChatTest(unittest.TestCase):
    def setUp(self):
        fd, self.db = tempfile.mkstemp(suffix=".db")
        os.close(fd)
        os.remove(self.db)  # let the tool create it fresh

    def tearDown(self):
        for suffix in ("", "-wal", "-shm"):
            try:
                os.remove(self.db + suffix)
            except OSError:
                pass

    def run_cli(self, *args, identity=None, extra_env=None):
        cmd = [sys.executable, CLI]
        if identity:
            cmd += ["--from", identity]
        cmd += list(args)
        env = {**os.environ, "AGENT_CHAT_DB": self.db, **(extra_env or {})}
        return subprocess.run(cmd, capture_output=True, text=True, env=env)

    def question_id_for(self, identity):
        out = self.run_cli("inbox", "--json", identity=identity)
        rows = json.loads(out.stdout)
        return rows[0]["id"] if rows else None

    def fake_aoe_env(self, mode="ok"):
        """A dir holding a fake `aoe`, plus env that puts it on PATH."""
        d = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, d, ignore_errors=True)
        shim = os.path.join(d, "aoe")
        with open(shim, "w") as f:
            f.write(FAKE_AOE)
        os.chmod(shim, 0o755)
        return {"PATH": d + os.pathsep + os.environ["PATH"],
                "AGENT_CHAT_FAKE_MODE": mode}

    # --- tests -------------------------------------------------------------
    def test_whoami_override(self):
        out = self.run_cli("whoami", identity=A)
        self.assertEqual(out.returncode, 0)
        self.assertIn("a1", out.stdout)
        self.assertIn("AgentA", out.stdout)

    def test_async_roundtrip(self):
        # A asks, times out fast -> pending
        ask = self.run_cli("ask", B, "what restitution?", "--timeout", "1",
                           "--no-doorbell", identity=A)
        self.assertEqual(ask.returncode, 3)  # 3 == no answer (pending)
        self.assertIn("pending", ask.stderr)

        # B sees it in inbox
        qid = self.question_id_for(B)
        self.assertIsNotNone(qid)

        # B replies
        rep = self.run_cli("reply", qid, "restitution=0.4", "--no-doorbell", identity=B)
        self.assertEqual(rep.returncode, 0)

        # question now answered -> drops off B's inbox
        self.assertIsNone(self.question_id_for(B))

        # A retrieves the reply, then it's consumed
        got = self.run_cli("replies", identity=A)
        self.assertIn("restitution=0.4", got.stdout)
        again = self.run_cli("replies", identity=A)
        self.assertIn("no new replies", again.stdout)

    def test_blocking_poll_returns_reply(self):
        # B replies ~1.5s after A starts a 6s blocking ask; ask must return it.
        def delayed_reply():
            for _ in range(40):
                qid = self.question_id_for(B)
                if qid:
                    self.run_cli("reply", qid, "ANSWER-42", "--no-doorbell", identity=B)
                    return
                time.sleep(0.25)

        t = threading.Thread(target=delayed_reply)
        t.start()
        ask = self.run_cli("ask", B, "blocking?", "--timeout", "6", "--no-doorbell",
                           identity=A)
        t.join()
        self.assertEqual(ask.returncode, 0)
        self.assertIn("ANSWER-42", ask.stdout)
        self.assertNotIn("pending", ask.stderr)

    def test_thread_shows_q_and_a(self):
        self.run_cli("ask", B, "Q-BODY", "--timeout", "1", "--no-doorbell", identity=A)
        qid = self.question_id_for(B)
        self.run_cli("reply", qid, "A-BODY", "--no-doorbell", identity=B)
        out = self.run_cli("thread", qid, identity=A)
        self.assertIn("Q-BODY", out.stdout)
        self.assertIn("A-BODY", out.stdout)
        self.assertIn("[Q]", out.stdout)
        self.assertIn("[A]", out.stdout)

    def test_unknown_reply_id_errors(self):
        out = self.run_cli("reply", "deadbeef", "x", "--no-doorbell", identity=B)
        self.assertNotEqual(out.returncode, 0)
        self.assertIn("no question", out.stderr)

    def test_ask_json_pending(self):
        out = self.run_cli("ask", B, "q?", "--timeout", "1", "--no-doorbell", "--json",
                           identity=A)
        self.assertEqual(out.returncode, 3)
        obj = json.loads(out.stdout)
        self.assertEqual(obj["status"], "pending")
        self.assertIsNone(obj["reply"])
        self.assertTrue(obj["msg_id"])

    def test_ask_json_answered(self):
        def delayed_reply():
            for _ in range(40):
                qid = self.question_id_for(B)
                if qid:
                    self.run_cli("reply", qid, "JSON-ANS", "--no-doorbell", identity=B)
                    return
                time.sleep(0.25)

        t = threading.Thread(target=delayed_reply)
        t.start()
        out = self.run_cli("ask", B, "q?", "--timeout", "6", "--no-doorbell", "--json",
                           identity=A)
        t.join()
        self.assertEqual(out.returncode, 0)
        obj = json.loads(out.stdout)
        self.assertEqual(obj["status"], "answered")
        self.assertEqual(obj["reply"], "JSON-ANS")
        self.assertEqual(obj["from"], "AgentB")

    def test_no_revive_skips_when_undeliverable(self):
        # doorbell fails (simulated stopped recipient) + --no-revive -> skip, don't block
        out = self.run_cli("ask", B, "q?", "--no-revive", "--json", "--timeout", "30",
                           identity=A, extra_env={"AGENT_CHAT_DOORBELL_FAIL": "1"})
        self.assertEqual(out.returncode, 3)
        obj = json.loads(out.stdout)
        self.assertEqual(obj["status"], "skipped")
        self.assertIsNone(obj["reply"])

    def test_bare_filename_db_does_not_crash(self):
        # AGENT_CHAT_DB as a bare filename (dirname == "") must not raise from
        # os.makedirs; run in a temp cwd so the db lands there.
        d = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, d, ignore_errors=True)
        env = {**os.environ, "AGENT_CHAT_DB": "mail.db"}
        out = subprocess.run([sys.executable, CLI, "--from", A, "inbox", "--json"],
                             capture_output=True, text=True, env=env, cwd=d)
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertNotIn("Traceback", out.stderr)

    def test_broadcast_json_schema_matches_ask(self):
        # broadcast --json emits an array of the same per-ask objects ask --json
        # emits (status/msg_id/thread_id/reply), plus to_id/to_title to name each
        # recipient. Both recipients time out fast -> no `from`/`reply_id` keys.
        out = self.run_cli("broadcast", "work/team", "status?", "--timeout", "1",
                           "--no-doorbell", "--json", extra_env=self.fake_aoe_env())
        self.assertEqual(out.returncode, 3)  # nobody answered
        arr = json.loads(out.stdout)
        self.assertEqual({r["to_title"] for r in arr}, {"Recruit", "Scout"})
        for r in arr:
            self.assertEqual(set(r), {"status", "msg_id", "thread_id", "reply",
                                      "to_id", "to_title"})
            self.assertEqual(r["status"], "pending")
            self.assertIsNone(r["reply"])

    def test_old_schema_db_is_migrated_not_crashed(self):
        # A DB created before `asker_pid` existed must get the column added
        # (ALTER TABLE), not crash on the first insert with "no column named
        # asker_pid". Mirrors a live DB from an earlier version.
        import sqlite3
        con = sqlite3.connect(self.db)
        con.execute(
            "CREATE TABLE messages (id TEXT PRIMARY KEY, thread_id TEXT, from_id "
            "TEXT, from_title TEXT, to_id TEXT, to_title TEXT, kind TEXT, "
            "in_reply_to TEXT, body TEXT, status TEXT, created_at TEXT, "
            "blocking_until REAL)")
        con.commit()
        con.close()
        out = self.run_cli("ask", B, "q?", "--timeout", "1", "--no-doorbell",
                           identity=A)
        self.assertEqual(out.returncode, 3, out.stderr)  # pending, not a crash
        self.assertNotIn("asker_pid", out.stderr)
        self.assertNotIn("Traceback", out.stderr)
        cols = [r[1] for r in
                sqlite3.connect(self.db).execute("PRAGMA table_info(messages)")]
        self.assertIn("asker_pid", cols)

    def test_old_schema_db_concurrent_broadcast_migrates(self):
        # Concurrent broadcast workers hitting an old-schema DB must not race the
        # migration into a "duplicate column name: asker_pid" crash (idempotent
        # ALTER). This is the first-broadcast-after-upgrade path.
        import sqlite3
        con = sqlite3.connect(self.db)
        con.execute(
            "CREATE TABLE messages (id TEXT PRIMARY KEY, thread_id TEXT, from_id "
            "TEXT, from_title TEXT, to_id TEXT, to_title TEXT, kind TEXT, "
            "in_reply_to TEXT, body TEXT, status TEXT, created_at TEXT, "
            "blocking_until REAL)")
        con.commit()
        con.close()
        out = self.run_cli("broadcast", "work/team", "s?", "--timeout", "1",
                           "--no-doorbell", "--json", extra_env=self.fake_aoe_env())
        self.assertEqual(out.returncode, 3, out.stderr)  # nobody answered, no crash
        self.assertNotIn("duplicate column", out.stderr)
        self.assertNotIn("Traceback", out.stderr)

    def test_empty_profile_nonjson_dies_cleanly(self):
        # aoe printing its plain-text "No sessions found" (exit 0) must surface as
        # a clean die(), not a raw JSONDecodeError traceback.
        out = self.run_cli("ask", "Recruit", "q?", "--timeout", "1", "--no-doorbell",
                           identity=A, extra_env=self.fake_aoe_env(mode="nonjson"))
        self.assertNotEqual(out.returncode, 0)
        self.assertNotIn("Traceback", out.stderr)
        self.assertIn("did not return JSON", out.stderr)


if __name__ == "__main__":
    unittest.main()
