import { beforeEach, describe, expect, it } from 'vitest';

import {
  clearStartingQuestionsCache,
  getCachedQuestions,
  setCachedQuestions,
} from './startingQuestionsCache';

beforeEach(() => {
  clearStartingQuestionsCache();
});

describe('startingQuestionsCache (AIP-SQ, кап B11)', () => {
  it('хранит и отдаёт вопросы по ключу; пустой массив — валидный кэш-хит', () => {
    setCachedQuestions('A.md', ['q1', 'q2']);
    setCachedQuestions('B.md', []);
    expect(getCachedQuestions('A.md')).toEqual(['q1', 'q2']);
    expect(getCachedQuestions('B.md')).toEqual([]); // не undefined → LLM не дёргаем повторно
    expect(getCachedQuestions('C.md')).toBeUndefined();
  });

  // audit B11: за длинную сессию кэш не должен расти неограниченно — кап эвиктит самую старую запись.
  it('кап эвиктит старейшую запись при переполнении (>200)', () => {
    for (let i = 0; i < 200; i++) setCachedQuestions(`n${i}.md`, [`q${i}`]);
    expect(getCachedQuestions('n0.md')).toEqual(['q0']); // ещё на месте (ровно 200)

    setCachedQuestions('n200.md', ['q200']); // 201-я → вытесняет старейшую (n0)
    expect(getCachedQuestions('n0.md')).toBeUndefined(); // эвикнута
    expect(getCachedQuestions('n200.md')).toEqual(['q200']); // новая на месте
    expect(getCachedQuestions('n1.md')).toEqual(['q1']); // следующая старейшая ещё жива
  });

  it('переустановка ключа двигает его в «свежие» (не эвиктится первым)', () => {
    for (let i = 0; i < 200; i++) setCachedQuestions(`n${i}.md`, [`q${i}`]);
    setCachedQuestions('n0.md', ['fresh']); // обновили старейший → теперь он самый свежий
    setCachedQuestions('n200.md', ['q200']); // переполнение → вытесняет n1 (новый старейший), не n0
    expect(getCachedQuestions('n0.md')).toEqual(['fresh']);
    expect(getCachedQuestions('n1.md')).toBeUndefined();
  });
});
