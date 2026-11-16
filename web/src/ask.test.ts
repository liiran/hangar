import { describe, expect, it } from "vitest";
import { answerOf, answersOf, NOTHING, pick, planOf, questionsOf, toggleOther, type, type Question } from "./ask";

const option = (label: string) => ({ label, description: `${label}!` });
const colour = { question: "Which colour?", header: "Colour", multiSelect: false, options: ["Red", "Green", "Blue"].map(option) };
const fruit = { question: "Which fruits?", header: "Fruit", multiSelect: true, options: ["Apple", "Banana", "Cherry"].map(option) };

describe("questionsOf", () => {
  it("reads the questions Claude Code sends", () => {
    const [one, many] = questionsOf({ questions: [colour, { ...fruit, options: [{ label: "Apple", description: "a", preview: "+--+" }, { label: "Banana" }] }] })!;
    expect(one).toEqual({ question: "Which colour?", header: "Colour", multi: false, options: [{ label: "Red", description: "Red!", preview: null }, { label: "Green", description: "Green!", preview: null }, { label: "Blue", description: "Blue!", preview: null }] });
    expect(many.multi).toBe(true);
    expect(many.options).toEqual([
      { label: "Apple", description: "a", preview: "+--+" },
      { label: "Banana", description: "", preview: null },
    ]);
  });

  it("is null for any other shape, so the raw request is shown instead of a half-drawn card", () => {
    const bad = [
      null,
      "questions",
      {},
      { questions: [] },
      { questions: "Which?" },
      { questions: [{ ...colour, question: "  " }] },
      { questions: [{ ...colour, options: [] }] },
      { questions: [{ ...colour, options: "Red" }] },
      { questions: [{ ...colour, options: [{ label: 5 }] }] },
      { questions: [{ ...colour, options: [option("Same"), option("Same")] }] }, // an answer could not tell them apart
      { questions: [colour, { ...fruit, question: colour.question }] }, // answers go back by question text
      { questions: Array.from({ length: 9 }, (_, i) => ({ ...colour, question: `Q${i}?` })) },
    ];
    for (const input of bad) expect(questionsOf(input), JSON.stringify(input)).toBeNull();
  });
});

describe("answering", () => {
  const [one, many] = questionsOf({ questions: [colour, fruit] }) as Question[];

  it("keeps one choice for a single-choice question, and the typed answer replaces it", () => {
    let p = pick(one, NOTHING, "Red");
    p = pick(one, p, "Green");
    expect(answerOf(one, p)).toBe("Green");
    p = type(one, p, "  Teal, like the sea ");
    expect([p.labels, answerOf(one, p)]).toEqual([[], "Teal, like the sea"]);
    // Choosing an option again puts the typed answer aside without throwing it away.
    p = pick(one, p, "Blue");
    expect([p.other, answerOf(one, p)]).toEqual(["  Teal, like the sea ", "Blue"]);
    p = toggleOther(one, p);
    expect(answerOf(one, p)).toBe("Teal, like the sea");
    expect(answerOf(one, type(one, p, "   "))).toBeNull();
  });

  it("joins several choices in Claude's order, the typed one last", () => {
    let p = pick(many, NOTHING, "Cherry");
    p = pick(many, p, "Apple");
    expect(answerOf(many, p)).toBe("Apple, Cherry");
    p = type(many, p, "Mango");
    expect(answerOf(many, p)).toBe("Apple, Cherry, Mango");
    p = pick(many, p, "Apple");
    expect(answerOf(many, p)).toBe("Cherry, Mango");
    p = toggleOther(many, p);
    expect([p.other, answerOf(many, p)]).toEqual(["Mango", "Cherry"]);
    expect(answerOf(many, pick(many, p, "Cherry"))).toBeNull();
  });

  it("has answers only once every question has one", () => {
    const first = pick(one, NOTHING, "Red");
    expect(answersOf([one, many], [first, NOTHING])).toBeNull();
    expect(answersOf([one, many], [first])).toBeNull();
    expect(answersOf([one, many], [first, type(many, NOTHING, "none of them")])).toEqual({ "Which colour?": "Red", "Which fruits?": "none of them" });
  });
});

describe("planOf", () => {
  it("reads the plan and where Claude saved it", () => {
    expect(planOf({ plan: "# Plan\n1. Do it", planFilePath: "C:\\Users\\me\\.claude\\plans\\p.md" })).toEqual({ plan: "# Plan\n1. Do it", file: "C:\\Users\\me\\.claude\\plans\\p.md" });
    expect(planOf({ plan: "x" })).toEqual({ plan: "x", file: null });
    for (const input of [null, {}, { plan: "  " }, { plan: ["a"] }, "plan"]) expect(planOf(input)).toBeNull();
  });
});
