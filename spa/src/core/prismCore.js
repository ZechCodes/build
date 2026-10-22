// Evaluate the CommonJS core in its own ESM dependency before highlight.js's
// side-effect grammar imports. Grammars read the global installed by core;
// importing core directly beside them can defer its lazy initializer until
// after the grammars in a production chunk graph. Core alone avoids Prism's
// automatic DOM highlighting.
import Prism from "prismjs/components/prism-core";

export default Prism;
