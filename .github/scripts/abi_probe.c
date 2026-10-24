/* The ABI probe release_guard.py (in the Forge) compiles against the Swift bundle's
 * hypertabular.h and links with one committed payload at a time, so each process loads or
 * links exactly one library. EXPECTED_VERSION arrives on the command line as the packed
 * MAJOR << 16 | MINOR << 8 | PATCH that hypertabular_version() returns. A non-zero exit is a
 * mismatch between the header, the library and the version being released.
 *
 * The body is HyperTabular's own: the version, the structure layouts the bindings reproduce by
 * hand (hypertabular_num_format, hypertabular_column_spec, the delimited state the caller
 * allocates), a delimited read through init and fill with the result codes and verdict reasons
 * the bindings switch on, a culture-formatted currency column, and the contract error for a
 * format whose size field is wrong. */
#include <stdint.h>
#include <string.h>
#include "hypertabular.h"
int main(void) {
    if(hypertabular_version()!=EXPECTED_VERSION) return 1;
    if(sizeof(hypertabular_num_format)!=92 || sizeof(hypertabular_column_spec)!=104) return 2;
    hypertabular_delimited_state state;
    if(hypertabular_delimited_state_size()!=sizeof(state)) return 3;
    hypertabular_dialect dialect={',',1,0,0};
    if(hypertabular_delimited_init(&state,&dialect)!=HYPERTABULAR_OK) return 4;
    hypertabular_column_spec spec={0}; spec.door=4;
    int32_t values[2]={0,0};
    hypertabular_cell_verdict verdicts[2]={{0}};
    hypertabular_column_buffer columns={values,verdicts};
    uint8_t arena[64]; hypertabular_span cells[8];
    hypertabular_buffers buffers={0};
    buffers.arena=arena; buffers.arena_cap=sizeof(arena);
    buffers.cells=cells; buffers.cells_cap=8;
    hypertabular_filled filled={0};
    const uint8_t input[]="123\nnope\n";
    if(hypertabular_delimited_fill(&state,input,sizeof(input)-1,1,&spec,&columns,1,2,&buffers,&filled)!=0) return 5;
    if(filled.rows!=2 || values[0]!=123 || verdicts[0].reason!=0 || verdicts[1].reason!=2) return 6;
    dialect.separator=';';
    if(hypertabular_delimited_init(&state,&dialect)!=0) return 7;
    spec.door=11; spec.format.size=sizeof(spec.format); spec.format.flags=65;
    spec.format.decimal_sep.bytes[0]=','; spec.format.decimal_sep.len=1;
    spec.format.group_sep.bytes[0]='.'; spec.format.group_sep.len=1;
    spec.format.currency_decimal_sep.bytes[0]='.'; spec.format.currency_decimal_sep.len=1;
    spec.format.currency_group_sep.bytes[0]=','; spec.format.currency_group_sep.len=1;
    spec.format.currency.bytes[0]='$'; spec.format.currency.len=1;
    double amount=0; hypertabular_cell_verdict amount_verdict={0};
    columns.values=&amount; columns.verdicts=&amount_verdict;
    const uint8_t currency_input[]="$1,234.50\n";
    if(hypertabular_delimited_fill(&state,currency_input,sizeof(currency_input)-1,1,&spec,&columns,1,1,&buffers,&filled)!=0) return 8;
    if(filled.rows!=1 || amount!=1234.5 || amount_verdict.reason!=0) return 9;
    if(hypertabular_delimited_init(&state,&dialect)!=0) return 10;
    spec=(hypertabular_column_spec){0}; spec.door=4; spec.format.size=1;
    columns.values=values; columns.verdicts=verdicts;
    if(hypertabular_delimited_fill(&state,input,sizeof(input)-1,1,&spec,&columns,1,2,&buffers,&filled)!=HYPERTABULAR_ERR_CONTRACT) return 11;
    return 0;
}
