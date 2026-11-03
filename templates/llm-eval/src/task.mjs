// The task Ratchet optimizes. TODO: replace the placeholder with your real LLM call (prompt, model, parameters).
//
// `run(input)` returns the answer, or { output, cost_usd, tokens } so the evaluator can report cost and token use:
//
//   const res = await client.messages.create({ model: MODEL, max_tokens: 64, messages: [{ role: 'user', content: PROMPT + input }] });
//   return { output: res.content[0].text, cost_usd: priceOf(res.usage), tokens: res.usage.input_tokens + res.usage.output_tokens };
//
// The placeholder below is a deterministic keyword classifier (sentiment: positive | negative | neutral) so that the
// template runs end to end without a network or an API key. It scores about 0.6 on eval/dataset.jsonl.

const POSITIVE = ['great', 'love', 'excellent', 'happy', 'fast', 'recommend'];
const NEGATIVE = ['bad', 'terrible', 'slow', 'broken', 'refund', 'disappointed'];

export async function run(input) {
  const text = String(input).toLowerCase();
  const pos = POSITIVE.filter((w) => text.includes(w)).length;
  const neg = NEGATIVE.filter((w) => text.includes(w)).length;
  const output = pos > neg ? 'positive' : neg > pos ? 'negative' : 'neutral';
  return { output, cost_usd: 0.0001, tokens: Math.ceil(text.length / 4) };
}
