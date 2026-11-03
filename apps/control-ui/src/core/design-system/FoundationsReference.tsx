import { mixrelayVariables, themeRoles } from '../theme/tokens';

export function TokenReference() {
  return (
    <>
      <table>
        <caption>themeRoles</caption>
        <thead>
          <tr>
            <th scope="col">역할</th>
            <th scope="col">Light</th>
            <th scope="col">Dark</th>
          </tr>
        </thead>
        <tbody>
          {Object.entries(themeRoles.light).map(([role, value]) => (
            <tr key={role}>
              <th scope="row">
                <code>{role}</code>
              </th>
              <td>
                <code>{value}</code>
              </td>
              <td>
                <code>{themeRoles.dark[role as keyof typeof themeRoles.dark]}</code>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <table>
        <caption>mixrelayVariables</caption>
        <thead>
          <tr>
            <th scope="col">CSS 변수</th>
            <th scope="col">값</th>
          </tr>
        </thead>
        <tbody>
          {Object.entries(mixrelayVariables).map(([name, value]) => (
            <tr key={name}>
              <th scope="row">
                <code>{name}</code>
              </th>
              <td>
                <code>{value}</code>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </>
  );
}
