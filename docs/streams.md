# Streams

`streams/` pays out tokens linearly per second, like Sablier and Drips. It is
used for contributor pay and grants.

## Model

The funder deposits the full `total` when the stream is created. At time `t`:

```
streamed(t) = 0                                            if t < cliff
            = total                                        if t ≥ end
            = mul_div_floor(total, t − start, end − start) otherwise
```

The recipient can withdraw `streamed(now) − withdrawn` at any time. Every
step rounds down, so the total paid out can never exceed the deposit. The
`prop_withdrawn_never_exceeds_deposit` test checks this over random
schedules.

## API

| Function | Who | Notes |
|----------|-----|-------|
| `initialize(governance)` | once | `governance` can cancel any stream. |
| `create_stream(funder, recipient, token, total, start, cliff, end) -> id` | funder | Pulls `total` right away. Needs `start < end` and `start ≤ cliff ≤ end`. `start` may be in the past, in which case the elapsed share is available at once. |
| `withdraw(id, amount)` | recipient | `amount` can be at most the accrued balance. |
| `cancel(caller, id)` | funder or governance | Pays the recipient what has accrued and not been withdrawn (nothing before the cliff), and refunds the rest to the funder. The stream is then frozen. |
| `transfer(id, new_recipient)` | recipient | Unwithdrawn accruals move with the stream. |
| `balance_of(id) -> (recipient, funder)` | view | What the recipient could withdraw now, and what the funder would get back if the stream were cancelled now. |
| `get_stream(id)`, `get_stream_count()` | view | |

## Example

```bash
# 12-month grant of 120 000 USDC (7 decimals) with a 3-month cliff
stellar contract invoke --id $STREAMS --source treasury -- create_stream \
  --funder $TREASURY --recipient $DEV --token $USDC --total 1200000000000 \
  --start 1767225600 --cliff 1775001600 --end 1798761600
```
