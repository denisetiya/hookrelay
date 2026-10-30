import http from 'k6/http';
import { check } from 'k6';

export const options = {
  scenarios: {
    ingest: {
      executor: 'constant-vus',
      vus: Number(__ENV.VUS || 20),
      duration: __ENV.DURATION || '60s',
    },
  },
};

const base = __ENV.HOOKRELAY_URL || 'http://localhost:8090';
const key = __ENV.HOOKRELAY_API_KEY;

export default function () {
  const id = `${__VU}-${__ITER}-${Date.now()}`;
  const response = http.post(
    `${base}/v1/events`,
    JSON.stringify({ type: 'load.test', payload: { id } }),
    {
      headers: {
        Authorization: `Bearer ${key}`,
        'Content-Type': 'application/json',
        'Idempotency-Key': id,
      },
    },
  );

  check(response, {
    accepted: (result) => result.status === 201 || result.status === 200,
  });
}
