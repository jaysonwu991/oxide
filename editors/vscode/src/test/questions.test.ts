// The question path is the one the extension cannot test against a live model:
// the request the `ask` tool emits is parsed here, the card's heading and
// settled label are what it shows, and the frame is what the CLI reads to
// answer it — the same shapes the desktop app's `resolve_question` carries.

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  questionAnswers,
  questionLabel,
  questionRequest,
  questionTitle,
  type Question,
} from "../core/questions";
import { questionFrame } from "../core/rpc";

const ask = (overrides: Partial<Question> = {}): Question => ({
  question: "Which database?",
  header: "",
  options: [],
  multiSelect: false,
  ...overrides,
});

describe("questionRequest", () => {
  it("reads the id and questions the CLI emits", () => {
    const request = questionRequest({
      type: "question_request",
      id: 4,
      questions: [
        {
          question: "Which database?",
          header: "Database",
          options: [
            { label: "Postgres", description: "Relational" },
            { label: "SQLite" },
          ],
          multiSelect: true,
        },
      ],
    });
    assert.deepEqual(request, {
      id: 4,
      questions: [
        {
          question: "Which database?",
          header: "Database",
          options: [
            { label: "Postgres", description: "Relational" },
            { label: "SQLite", description: "" },
          ],
          multiSelect: true,
        },
      ],
    });
  });

  it("keeps a free-text question, and one that asks for several answers", () => {
    const request = questionRequest({
      type: "question_request",
      id: 1,
      questions: [
        { question: "What should the package be called?" },
        { question: "Which targets?", multiSelect: true, options: ["linux"] },
      ],
    });
    assert.equal(request?.questions.length, 2);
    assert.deepEqual(request?.questions[0], ask({ question: "What should the package be called?" }));
    assert.deepEqual(request?.questions[1], {
      question: "Which targets?",
      header: "",
      options: [{ label: "linux", description: "" }],
      multiSelect: true,
    });
  });

  // The core parses options written as plain strings (`ask::choices`), so a
  // request from an older CLI still paints its choices.
  it("accepts plain-string options", () => {
    assert.deepEqual(
      questionRequest({ type: "question_request", id: 2, questions: [{ question: "Pick", options: ["a", "b"] }] })
        ?.questions[0].options,
      [
        { label: "a", description: "" },
        { label: "b", description: "" },
      ],
    );
  });

  // A card whose answers could never reach the broker would hold the turn until
  // the request times out, so a request without an id or without a question is
  // dropped instead.
  it("refuses a request without a usable id or question", () => {
    for (const id of [undefined, null, 0, -1, "3", Number.NaN]) {
      assert.equal(
        questionRequest({ type: "question_request", id, questions: [{ question: "Pick" }] }),
        null,
      );
    }
    assert.equal(questionRequest({ type: "question_request", id: 1 }), null);
    assert.equal(questionRequest({ type: "question_request", id: 1, questions: [{ header: "H" }] }), null);
    assert.equal(questionRequest({ type: "question_request", id: 1, questions: "Pick one" }), null);
  });

  it("skips the entries it cannot paint and keeps the rest", () => {
    const request = questionRequest({
      type: "question_request",
      id: 9,
      questions: [
        { question: "  " },
        { question: "Which one?", options: [null, { description: "no label" }, { label: "  " }, { label: " yes " }] },
      ],
    });
    assert.deepEqual(request, {
      id: 9,
      questions: [ask({ question: "Which one?", options: [{ label: "yes", description: "" }] })],
    });
  });
});

describe("questionAnswers", () => {
  it("keeps the question each answer belongs to, ticked labels first", () => {
    assert.deepEqual(
      questionAnswers([
        { question: "Which database?", values: ["Postgres", " Postgres ", "SQLite", ""] },
        { question: "Anything else?", values: [] },
      ]),
      [
        { question: "Which database?", values: ["Postgres", "SQLite"] },
        { question: "Anything else?", values: [] },
      ],
    );
  });

  it("drops an entry that names no question, and reads non-strings as nothing", () => {
    assert.deepEqual(questionAnswers([{ values: ["x"] }, { question: "Q", values: [1, null, {}] }]), [
      { question: "Q", values: [] },
    ]);
    assert.deepEqual(questionAnswers([{ question: "Q", values: "one" }]), [{ question: "Q", values: [] }]);
  });

  it("reads an empty list as no answers at all, which is a dismissal", () => {
    assert.deepEqual(questionAnswers([]), []);
  });
});

describe("question labels", () => {
  it("heads the card with the first question's own header, else the question", () => {
    assert.equal(questionTitle([ask({ header: "Database" })]), "Database");
    assert.equal(questionTitle([ask()]), "Which database?");
    assert.equal(questionTitle([]), "Question");
  });

  it("summarizes what was answered, and says when nothing was", () => {
    assert.equal(questionLabel([{ question: "Q", values: ["Postgres", "SQLite"] }]), "Postgres, SQLite");
    assert.equal(
      questionLabel([
        { question: "A", values: ["one"] },
        { question: "B", values: ["two"] },
      ]),
      "one · two",
    );
    assert.equal(questionLabel([{ question: "Q", values: [] }]), "Dismissed without an answer");
    assert.equal(questionLabel([]), "Dismissed without an answer");
  });
});

describe("question frame", () => {
  const parsed = (frame: string): unknown => JSON.parse(frame.trim());

  it("answers a question by id, on one line", () => {
    const frame = questionFrame(3, [
      { question: "Which database?", values: ["Postgres", "because it is here"] },
    ]);
    assert.equal(frame.endsWith("\n"), true);
    assert.equal(frame.indexOf("\n"), frame.length - 1);
    assert.deepEqual(parsed(frame), {
      type: "question",
      id: 3,
      answers: [{ question: "Which database?", values: ["Postgres", "because it is here"] }],
    });
  });

  it("sends an empty answer list as a dismissal, never an omitted field", () => {
    assert.deepEqual(parsed(questionFrame(5, [])), { type: "question", id: 5, answers: [] });
  });
});
