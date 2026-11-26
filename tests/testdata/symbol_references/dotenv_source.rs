// <block same-as="tests/testdata/symbol_references/dotenv_target.env#/EMAIL_FROM" same-as-pattern='(?:= "|^)(?P<value>[^"\n]+)(?:";|$)'>
const EMAIL_FROM: &str = "Example <noreply@example.com>";
// </block>

// <block same-as="tests/testdata/symbol_references/dotenv_target.env#/VITE_API_URL" same-as-pattern='(?:= "|^)(?P<value>[^"\n]+)(?:";|$)'>
const VITE_API_URL: &str = "https://api.example.com";
// </block>

// <block same-as="tests/testdata/symbol_references/dotenv_target.env#/MESSAGE" same-as-pattern="(?P<value>hello|world)">
const MESSAGE: &str = "hello
world";
// </block>

// <block same-as="tests/testdata/symbol_references/dotenv_target.env#/RAW" same-as-pattern='(?:= r"|^)(?P<value>[^"\n]+)(?:";|$)'>
const RAW: &str = r"literal \n $HOME";
// </block>

// <block same-as="tests/testdata/symbol_references/dotenv_target.env#/TEMPLATE" same-as-pattern="(?P<value>line [12])">
const TEMPLATE: &str = "line 1
line 2";
// </block>

// <block same-as="tests/testdata/symbol_references/.env.example#/LOG_RETENTION_DAYS" same-as-pattern="(?:= |^)(?P<value>\d+)(?:;|$)">
const LOG_RETENTION_DAYS: u32 = 90;
// </block>

// <block same-as="tests/testdata/symbol_references/.env.local#/APP_ENV" same-as-pattern='(?:= "|^)(?P<value>[^"\n]+)(?:";|$)'>
const APP_ENV: &str = "local";
// </block>
