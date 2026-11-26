import sys
# times (s) of events from the decoded capture, ordered
ts=[]
for l in open(sys.argv[1]):
    p=l.split()
    if p and p[0]=='event': ts.append(float(p[3]))
n=len(ts)
for f in (250e6, 64e6):
  for bits in (14,16,18,20):
    thr=(3<<bits)//4; ref=None; refs=0
    for t in ts:
        c=int(t*f)
        if ref is None or c-ref>thr: refs+=1; ref=c
    print(f"{f/1e6:.0f} MHz stamp {bits} bits: refs/event {refs/n:.3f}  ({refs/(ts[-1]-ts[0]):.0f}/s)")
print(n, n/(ts[-1]-ts[0]), "ev/s")
