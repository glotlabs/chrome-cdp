// What every frame and worker of the target matrix (plan 13) does when
// asked: `targetOps(who, op)` answers with a value to post back.
self.targetOps = async (who, op) => {
  switch (op) {
    case 'ping':
      return 'pong';
    case 'log':
      console.log('log from ' + who);
      return 'logged';
    case 'throw':
      setTimeout(() => { throw new Error('thrown in ' + who); });
      return 'thrown';
    case 'fetch': {
      const response = await fetch('/targets/api?from=' + encodeURIComponent(who));
      return (await response.json()).from;
    }
    case 'init':
      // What the matrix's init script set, if it ran here.
      return self.matrixInit ?? null;
    case 'report':
      return {
        userAgent: navigator.userAgent,
        language: navigator.language,
        timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
        now: Date.now(),
        random: Math.random(),
      };
    default:
      return 'unknown op ' + op;
  }
};
